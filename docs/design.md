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

## Pairing

A back end usually admits the devices in a `PeerSet`. Opened on a file, the set
is written through on every change (one key per line, the file replaced
atomically, mode 0600), so pairings survive restarts. Devices join it by
redeeming a ticket:

1. **Issue.** The back end calls `Pairing::issue(addr, ttl)`. A ticket is the
   back end's address (key, sockets, relay) plus a random 128-bit token that
   works once, before `ttl` runs out (at most a day). Tokens live only in the
   back end's memory, so a restart withdraws them all.
2. **Carry.** The ticket travels out of band, normally as a QR code on a screen
   next to the back end. Its text stays within a QR code's alphanumeric set.
   Reading it back accepts lower case and ignores whitespace.
3. **Redeem.** The front end dials the key in the ticket, so the handshake
   proves it reached that back end and not an impostor. It then calls `pair`
   on the reserved application `pair` with the token as its credential
   (`Node::pair`).
4. **Admit.** The back end spends the token and adds the key the handshake
   proved. From the next request on, that key is admitted like any paired peer.

```
ticket = "KURI-LINK:" base32(CBOR map)     base32: RFC 4648 alphabet, no padding
map    = {"v": 1, "id": bytes(32), "ip": ["addr:port", …]?, "relay": ["url", …]?,
          "token": bytes(16)}
```

What keeps it tight:

- **One door.** The token admits its holder to method `pair` of application
  `pair` and nothing else. Presented anywhere else, it is refused and stays
  unspent.
- **One use.** Presented at that door, the token is spent whether or not the
  pairing then succeeds, so a ticket admits one device at most.
- **A secret until used.** Whoever presents the ticket first gets in. Keep the
  TTL short (a minute or two), show the code only when asked, and withdraw it
  (`revoke`, `revoke_all`) when the code is dismissed.
- **Unpairing is immediate.** Remove the key from the `PeerSet`. Authorization
  runs per request, so the device is refused from its next request, even on a
  connection already open.
- **It composes.** `NodeBuilder::pairing` guards the pairing door itself. The
  node's other applications admit the paired set, unless
  `NodeBuilder::authorizer` names an authorizer of its own, which may consult
  the set (`Pairing::peers`) or ignore it.
- **One way.** The back end learns the front end's key; the front end keeps the
  address `Node::pair` returns. Two nodes that serve each other pair twice.

## Hooks

| Hook | Default | Purpose |
|---|---|---|
| key (`secret_key`, `load_or_create_key`) | ephemeral key | the node's identity |
| `Authorizer` | `DenyAll`, or the paired set with pairing on | who may call what |
| `Pairing` | off | let devices join by one-time ticket |
| `Relays` | `None` | which relays may carry traffic |
| `Waker` | none | wake a sleeping back end before one retry |

## Not yet

- Typed Dart clients generated from Rust service definitions. Today the Dart
  side calls by method name with CBOR bytes.
- A server-streaming helper. Today it is built on `open`.
- Local-network discovery (mDNS). Today addresses are passed explicitly.
- Browser front ends. These could only ever go through a relay, because browsers
  cannot open raw UDP.
