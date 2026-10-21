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
addr:  C→S Header{v:1, op:"open", method:"kuri-link.addr", cred?} [FIN]
       S→C Reply{"ok": null}, then frames {"id": bytes(32), "ip": ["addr:port", …]?,
       "relay": ["url", …]?}: the back end's address now, then each change
```

- **One reserved method.** Every application a back end serves answers
  `kuri-link.addr` itself, without its service seeing it: an `open` whose
  stream carries the back end's own address, then each change of it, at most
  one a second. A client asks once it makes its first request, with that
  request's credential, so the authorizer decides as for any method, and
  decides again before each change is sent. A back end from before this method
  answers `no_such_method`, and the client stops asking.

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

## Reaching a back end from outside

A home back end is behind the home router's NAT, and its public IP is not
fixed. By default it opens nothing: every connection is dialled out, so a
device away from home reaches it only through a relay. A port forwarded by the
router is an optimisation on top, a direct path with no relay in between, and
it is the owner's choice, because it is an inbound port into the home: with
`NodeBuilder::port_mapping(true)`, the router holds a port that leads to the
back end's machine for as long as the node runs. Off, no node sends the router
a NAT-PMP, UPnP or PCP request, iroh's own port mapping included.

Turned on:

- **The back end asks the router to forward its port**, by NAT-PMP (RFC 6886)
  or else UPnP, renewing every two minutes. Home routers grant this in their
  default settings. The outside address joins the node's own addresses, so
  tickets carry it, and a front end on mobile data dials it directly.
- **Only mappings that lapse.** Each is asked for two hours and renewed;
  `Node::shutdown` drops it, and one left by a crash lapses on its own. A
  router that grants only permanent mappings is not used: such a mapping
  outlives the node, until the router restarts, and by then the local address
  it leads to may belong to another device.
- **It asks for the outside port to equal its own.** iroh can map a port by
  itself, but it lets the router choose the outside port, and a router
  chooses a new one each time the node starts: measured on a home router, six
  starts on one local port gave six outside ports. Asked for its own port, the
  same router granted it, and granted it again after a restart. With a kept
  port (`NodeBuilder::port`), a restart no longer changes the back end's
  public address. iroh's own mapping is therefore off on a node that serves
  an application; a node that only calls uses it.
- **It says when it cannot be reached.** `Node::reach` reports `Public`,
  `BehindNat` (the router's own outside address is not public: carrier-grade
  NAT, or a second router) or `Unmapped` with the reason (mapping is off, or
  no router forwards the port). In the last two cases no direct path from
  outside exists; that is the network's shape, not a fault, and only a relay
  closes it. A front end holding only such addresses gives up after five
  seconds instead of iroh's thirty, and says that only a device on the back
  end's network can reach it.
- **Its front ends learn where it is.** A back end's address still changes
  when the ISP gives the home a new IP, or the router gives the back end a new
  address on the home network. Its front ends learn the new address over
  `kuri-link.addr` (see the wire format) while they are connected, and as soon
  as they connect again; `Client::peer_addr` is what they dial from then on,
  and what the app keeps.
- **At home, it announces itself.** A back end announces its key and
  addresses on the local network by mDNS (service `kuri-link`), and every node
  looks peers up that way when it dials. So a device back on the home network
  finds the back end whatever its address there has become. Front ends look
  up without announcing themselves.
- **What remains.** A device that is away from home, and not connected, while
  the home's public IP changes does not learn the new one until it is back on
  the home network. Nothing outside the home could tell it without a third
  party.

## Hooks

| Hook | Default | Purpose |
|---|---|---|
| key (`secret_key`, `load_or_create_key`) | ephemeral key | the node's identity |
| `Authorizer` | `DenyAll`, or the paired set with pairing on | who may call what |
| `Pairing` | off | let devices join by one-time ticket |
| `Relays` | `None` | which relays may carry traffic |
| `port` | random | the port a back end keeps, so its public address survives restarts |
| `port_mapping` | off | ask the home router to forward the port: an inbound port into the home |
| `local_discovery` | on | announce and look up nodes on the local network (mDNS) |
| `Waker` | none | wake a sleeping back end before one retry |

## Not yet

- Typed Dart clients generated from Rust service definitions. Today the Dart
  side calls by method name with CBOR bytes.
- A server-streaming helper. Today it is built on `open`.
- Browser front ends. These could only ever go through a relay, because browsers
  cannot open raw UDP.
