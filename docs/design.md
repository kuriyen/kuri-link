# Kuri Link design

## Layers

```
front end (Dart or Rust)                    back end (Rust)
        │ Client::call / open                     │ Service (e.g. Methods)
        ▼                                          ▲
   ┌──────────── Kuri Link: one request per stream ────────────┐
   │  in-process: direct call + in-memory pipe                  │
   │  peer:       QUIC bidirectional stream over iroh           │
   └────────────────────────────────────────────────────────────┘
        iroh: dial by key · TLS 1.3 raw public keys · hole punching · relays
```

## Back end location is configuration

A `Client` wraps one of two transports:

| Location | How | Authorization | Typical use |
|---|---|---|---|
| In-process | the app's Rust code calls `register_local(app, service)`; the front end uses `Client::local(app)` | none — the process is the trust boundary | offline mode; a desktop app with its back end linked in |
| Peer | `Node::connect(addr, app)` over iroh | the server's `Authorizer`, per request | a phone reaching a back end at home; two processes on one host (loopback) |

Both satisfy the same contract, enforced by one test suite run against each:
identical results, identical `Fault` codes, `open` streams with identical
end-of-stream behaviour. Two processes on the same host use the peer path over
loopback rather than a local socket, so the key check still applies across a
process or sandbox boundary.

## Wire format (version 1)

- **ALPN** `kuri-link/1/<app>` — one per application and wire version. A peer that
  does not serve the app, or speaks another version, refuses at the handshake.
- **One request per QUIC bidirectional stream.** No head-of-line blocking
  between requests; concurrency is QUIC's.
- **Frame**: 4-byte big-endian length, then CBOR. Default limit 1 MiB per frame;
  bulk data belongs on an `open` stream.

```
call:  C→S Header{v:1, op:"call", method, args, cred?} [FIN]   S→C Reply [FIN]
open:  C→S Header{v:1, op:"open", method, args, cred?}         S→C Reply
       Reply = {"ok": value} → stream becomes raw bytes both ways
       Reply = {"err": {...}} → server finishes the stream
Reply: {"ok": <value>} | {"err": {"code": <code>, "message": <text>}}
code:  bad_request | unsupported_version | denied | no_such_method | app
       (an unknown code decodes as `unknown`)
```

### Why CBOR maps with named keys

- **Evolution.** Unknown keys are ignored and missing optional keys default, so
  front ends and back ends can be upgraded independently — the normal case when
  a phone app and a home server update on different schedules.
- **Untyped front ends.** A UI described as data holds string-keyed maps; those
  map straight onto CBOR maps, so a front end with no generated code can still
  call any method.
- **Not postcard.** It is compact but not self-describing: adding a field breaks
  older readers.
- **Not integer keys.** They would save bytes, but cost the second point above.

## Security model

- **Identity.** A node's key is its address. The QUIC/TLS handshake authenticates
  both ends by key (raw public keys), end to end — a relay forwards ciphertext
  and cannot impersonate either side.
- **Authorization.** `Authorizer::authorize` runs for every request, with the
  proven peer key, app, method, and an opaque credential. Per request, so a
  revoked or expired credential stops working on the next call.
- **Holder-bound credentials need no extra challenge.** If a credential names
  the key it is issued to, the authorizer only checks that this key equals the
  connection's peer key. The handshake has already proven possession of that key.
- **Defaults are closed.** The `DenyAll` authorizer is the default, and relays
  are off unless chosen. Relay operators see metadata (who, when, how much) but
  never content.

## Hooks

| Hook | Default | Purpose |
|---|---|---|
| key (`secret_key`, `load_or_create_key`) | ephemeral key | the node's identity |
| `Authorizer` | `DenyAll` | who may call what |
| `Relays` | `None` | which relays may carry traffic |
| `Waker` | none | wake a sleeping back end before one retry |

## Not yet

- Typed Dart clients generated from Rust service definitions. Today the Dart
  side calls by method name with CBOR bytes.
- A server-streaming helper. Today it is built on `open`.
- Local-network discovery (mDNS). Today addresses are passed explicitly.
- Browser front ends. These could only ever go through a relay, because browsers
  cannot open raw UDP.
