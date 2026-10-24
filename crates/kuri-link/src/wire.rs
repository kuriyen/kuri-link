//! The wire format. One request per QUIC bidirectional stream; every message
//! on it is a frame: a 4-byte big-endian length, then that many bytes of CBOR.
//!
//! ```text
//! call:  client → Header{op: call}  [finish]      server → Reply  [finish]
//! open:  client → Header{op: open}                server → Reply
//!        then, if the reply was `ok`, the stream is raw bytes both ways
//! ```

use ciborium::Value;
use iroh::endpoint::{RecvStream, SendStream};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// The wire version. Part of the ALPN, so mismatched peers refuse each other
/// at the handshake; also carried in every header, so a mismatch that slips
/// through is refused per request.
pub const WIRE_VERSION: u32 = 1;

/// Largest frame either side reads by default: 1 MiB. Bulk data belongs on an
/// `open` stream, not in arguments.
pub const DEFAULT_MAX_FRAME: usize = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Op {
    Call,
    Open,
}

/// The first frame on every stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Header {
    pub v: u32,
    pub op: Op,
    pub method: String,
    #[serde(default = "null")]
    pub args: Value,
    /// Opaque to Kuri Link; handed to the [`crate::Authorizer`] as-is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cred: Option<serde_bytes::ByteBuf>,
    /// On a stream a gateway forwards to a back end: the application the
    /// front end asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// On a stream a gateway forwards: the front end's key, as the gateway
    /// vouches for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller: Option<serde_bytes::ByteBuf>,
    /// On a stream a gateway forwards: what the gateway's authorizer said of
    /// the caller, when it said anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claims: Option<Value>,
}

impl Header {
    pub(crate) fn new(op: Op, method: &str, args: Value, cred: Option<Vec<u8>>) -> Header {
        Header {
            v: WIRE_VERSION,
            op,
            method: method.to_owned(),
            args,
            cred: cred.map(serde_bytes::ByteBuf::from),
            app: None,
            caller: None,
            claims: None,
        }
    }
}

fn null() -> Value {
    Value::Null
}

/// The server's single reply frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Reply {
    Ok(Value),
    Err(Fault),
}

/// Why a request was refused or failed, as the caller sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct Fault {
    pub code: FaultCode,
    pub message: String,
}

impl Fault {
    pub(crate) fn new(code: FaultCode, message: impl Into<String>) -> Self {
        Fault {
            code,
            message: message.into(),
        }
    }
}

/// Fault categories. Flexible: a code a peer does not know decodes as
/// [`FaultCode::Unknown`] rather than failing the whole reply. More codes may
/// come, so a `match` on it needs a wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum FaultCode {
    /// The header was missing, malformed, or too large.
    BadRequest,
    /// The header's wire version is not one this server speaks.
    UnsupportedVersion,
    /// The authorizer refused the caller.
    Denied,
    /// The application serves no method by that name (for that operation).
    NoSuchMethod,
    /// The method ran and failed.
    App,
    /// The application's back end is not there now: a gateway answered, and
    /// no back end it admits has registered the application, or the one that
    /// had is gone.
    Unavailable,
    /// A code from a newer peer.
    #[serde(other)]
    Unknown,
}

/// A failure below the request layer: the stream, the encoding, or a limit.
/// More cases may come, so a `match` on it needs a wildcard arm.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WireError {
    #[error("stream: {0}")]
    Stream(String),
    #[error("encode: {0}")]
    Encode(String),
    #[error("decode: {0}")]
    Decode(String),
    #[error("frame of {0} bytes exceeds the limit")]
    TooLarge(usize),
}

pub(crate) async fn write_frame<T: Serialize>(
    send: &mut SendStream,
    msg: &T,
    max: usize,
) -> Result<(), WireError> {
    let mut buf = Vec::new();
    ciborium::into_writer(msg, &mut buf).map_err(|e| WireError::Encode(e.to_string()))?;
    if buf.len() > max {
        return Err(WireError::TooLarge(buf.len()));
    }
    let len = u32::try_from(buf.len()).map_err(|_| WireError::TooLarge(buf.len()))?;
    send.write_all(&len.to_be_bytes())
        .await
        .map_err(|e| WireError::Stream(e.to_string()))?;
    send.write_all(&buf)
        .await
        .map_err(|e| WireError::Stream(e.to_string()))?;
    Ok(())
}

pub(crate) async fn read_frame<T: DeserializeOwned>(
    recv: &mut RecvStream,
    max: usize,
) -> Result<T, WireError> {
    let mut len = [0u8; 4];
    recv.read_exact(&mut len)
        .await
        .map_err(|e| WireError::Stream(e.to_string()))?;
    let len = u32::from_be_bytes(len) as usize;
    if len > max {
        return Err(WireError::TooLarge(len));
    }
    let mut buf = vec![0u8; len];
    recv.read_exact(&mut buf)
        .await
        .map_err(|e| WireError::Stream(e.to_string()))?;
    decode(&buf)
}

/// One frame's CBOR, decoded as every frame read off the wire is.
fn decode<T: DeserializeOwned>(buf: &[u8]) -> Result<T, WireError> {
    ciborium::from_reader(buf).map_err(|e| WireError::Decode(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(entries: Vec<(&str, Value)>) -> Value {
        Value::Map(
            entries
                .into_iter()
                .map(|(k, v)| (Value::Text(k.into()), v))
                .collect(),
        )
    }

    fn text(s: &str) -> Value {
        Value::Text(s.into())
    }

    fn frame(value: Value) -> Vec<u8> {
        let mut buf = Vec::new();
        ciborium::into_writer(&value, &mut buf).unwrap();
        buf
    }

    // Version 1 promises that a peer from a later release can add optional
    // keys and fault codes, and that this one reads past them. Rejecting
    // unknown keys, or decoding by hand, would break that silently.
    #[test]
    fn a_newer_peers_keys_and_fault_codes_still_read() {
        let header: Header = decode(&frame(map(vec![
            ("v", Value::Integer(1.into())),
            ("op", text("call")),
            ("method", text("add")),
            ("args", Value::Null),
            ("priority", map(vec![])),
        ])))
        .unwrap();
        assert_eq!((header.v, header.op), (1, Op::Call));
        assert_eq!(header.method, "add");

        let fault = map(vec![
            ("code", text("rate_limited")),
            ("message", text("slow down")),
            ("retry_after", Value::Integer(5.into())),
        ]);
        let reply: Reply = decode(&frame(map(vec![("err", fault)]))).unwrap();
        let Reply::Err(fault) = reply else {
            panic!("expected an error reply, got {reply:?}");
        };
        assert_eq!(fault, Fault::new(FaultCode::Unknown, "slow down"));
    }
}
