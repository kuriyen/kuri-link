//! What an application implements, and a builder that covers the common case.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use ciborium::Value;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::hooks::Caller;
use crate::BoxFuture;

/// Both directions of an `open` stream, after the server accepted it.
///
/// The same type whether the back end is a peer (a QUIC stream) or linked
/// into this process (an in-memory pipe). Shutting down `send` is end-of-stream
/// for the other side's `recv`.
pub struct Duplex {
    pub send: Box<dyn AsyncWrite + Send + Unpin>,
    pub recv: Box<dyn AsyncRead + Send + Unpin>,
}

impl std::fmt::Debug for Duplex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Duplex")
    }
}

impl Duplex {
    /// An in-memory pair: what one end writes, the other reads.
    pub(crate) fn pipe(buffer: usize) -> (Duplex, Duplex) {
        let (a, b) = tokio::io::duplex(buffer);
        let (a_recv, a_send) = tokio::io::split(a);
        let (b_recv, b_send) = tokio::io::split(b);
        (
            Duplex {
                send: Box::new(a_send),
                recv: Box::new(a_recv),
            },
            Duplex {
                send: Box::new(b_send),
                recv: Box::new(b_recv),
            },
        )
    }
}

/// A method's failure, as the application reports it.
#[derive(Debug, Clone, thiserror::Error)]
pub enum AppError {
    #[error("no such method: {0}")]
    NoSuchMethod(String),
    #[error("bad arguments: {0}")]
    BadArgs(String),
    #[error("{0}")]
    Failed(String),
}

/// Runs an accepted `open` stream to completion.
pub type OpenHandler = Box<dyn FnOnce(Duplex) -> BoxFuture<'static, ()> + Send>;

/// An application's back end.
///
/// `open` is two-phase on purpose: [`Service::accept_open`] decides before
/// the stream is committed, so a refusal reaches the caller as a [`Fault`]
/// instead of a stream that silently closes.
///
/// [`Fault`]: crate::Fault
pub trait Service: Send + Sync + 'static {
    fn call(
        self: Arc<Self>,
        caller: Caller,
        method: String,
        args: Value,
    ) -> BoxFuture<'static, Result<Value, AppError>>;

    fn accept_open(
        self: Arc<Self>,
        caller: Caller,
        method: String,
        args: Value,
    ) -> BoxFuture<'static, Result<OpenHandler, AppError>> {
        let _ = (caller, args);
        Box::pin(async move { Err(AppError::NoSuchMethod(method)) })
    }

    /// The names of the methods this service answers, where it can list them.
    /// [`NodeBuilder::bind`](crate::NodeBuilder::bind) refuses a service that
    /// lists one starting with [`RESERVED_PREFIX`](crate::RESERVED_PREFIX). A
    /// request for such a name never reaches a service either way. The
    /// default lists none.
    fn method_names(&self) -> Vec<String> {
        Vec::new()
    }
}

/// Converts a serializable value to a CBOR [`Value`].
pub fn to_value<T: Serialize + ?Sized>(t: &T) -> Result<Value, AppError> {
    Value::serialized(t).map_err(|e| AppError::Failed(format!("encoding a result: {e}")))
}

/// Converts a CBOR [`Value`] to a typed value.
pub fn from_value<T: DeserializeOwned>(v: Value) -> Result<T, AppError> {
    v.deserialized()
        .map_err(|e| AppError::BadArgs(e.to_string()))
}

type UnaryFn =
    Arc<dyn Fn(Caller, Value) -> BoxFuture<'static, Result<Value, AppError>> + Send + Sync>;
type DuplexFn =
    Arc<dyn Fn(Caller, Value, Duplex) -> BoxFuture<'static, Result<(), AppError>> + Send + Sync>;

/// A [`Service`] assembled from closures, one per method.
///
/// ```no_run
/// # use kuri_link::{Methods, Value};
/// let methods = Methods::new()
///     .unary("echo", |_caller, args| async move { Ok(args) })
///     .typed("add", |_caller, (a, b): (i64, i64)| async move { Ok(a + b) });
/// ```
#[derive(Default, Clone)]
pub struct Methods {
    unary: HashMap<String, UnaryFn>,
    duplex: HashMap<String, DuplexFn>,
}

impl std::fmt::Debug for Methods {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Methods")
            .field("unary", &self.unary.keys().collect::<Vec<_>>())
            .field("duplex", &self.duplex.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Methods {
    pub fn new() -> Self {
        Self::default()
    }

    /// A method over untyped CBOR values.
    pub fn unary<F, Fut>(mut self, name: &str, f: F) -> Self
    where
        F: Fn(Caller, Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value, AppError>> + Send + 'static,
    {
        self.unary
            .insert(name.to_owned(), Arc::new(move |c, v| Box::pin(f(c, v))));
        self
    }

    /// A method with typed arguments and result, converted through CBOR.
    pub fn typed<A, R, F, Fut>(self, name: &str, f: F) -> Self
    where
        A: DeserializeOwned + Send + 'static,
        R: Serialize + Send + 'static,
        F: Fn(Caller, A) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<R, AppError>> + Send + 'static,
    {
        let f = Arc::new(f);
        self.unary(name, move |caller, args| {
            let f = f.clone();
            async move {
                let args: A = from_value(args)?;
                let out = f(caller, args).await?;
                to_value(&out)
            }
        })
    }

    /// A method that takes over a raw byte stream once accepted.
    pub fn duplex<F, Fut>(mut self, name: &str, f: F) -> Self
    where
        F: Fn(Caller, Value, Duplex) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), AppError>> + Send + 'static,
    {
        self.duplex.insert(
            name.to_owned(),
            Arc::new(move |c, v, d| Box::pin(f(c, v, d))),
        );
        self
    }
}

impl Service for Methods {
    fn call(
        self: Arc<Self>,
        caller: Caller,
        method: String,
        args: Value,
    ) -> BoxFuture<'static, Result<Value, AppError>> {
        match self.unary.get(&method).cloned() {
            Some(f) => f(caller, args),
            None => Box::pin(async move { Err(AppError::NoSuchMethod(method)) }),
        }
    }

    fn accept_open(
        self: Arc<Self>,
        caller: Caller,
        method: String,
        args: Value,
    ) -> BoxFuture<'static, Result<OpenHandler, AppError>> {
        let f = self.duplex.get(&method).cloned();
        Box::pin(async move {
            let f = f.ok_or_else(|| AppError::NoSuchMethod(method.clone()))?;
            let run: OpenHandler = Box::new(move |duplex| {
                Box::pin(async move {
                    if let Err(e) = f(caller, args, duplex).await {
                        tracing::debug!(%method, error = %e, "open stream ended with an error");
                    }
                })
            });
            Ok(run)
        })
    }

    fn method_names(&self) -> Vec<String> {
        self.unary
            .keys()
            .chain(self.duplex.keys())
            .cloned()
            .collect()
    }
}
