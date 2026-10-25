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
       | unavailable (an unknown code decodes as `unknown`)
addr:  C→S Header{v:1, op:"open", method:"kuri-link.addr", cred?} [FIN]
       S→C Reply{"ok": null}, then frames {"id": bytes(32), "ip": ["addr:port", …]?,
       "relay": ["url", …]?}: the back end's address now, then each change
```

- **Reserved method names.** Names starting with `kuri-link.` belong to the
  library, not to applications: a request for one never reaches a service,
  and a node refuses at `bind` a service that lists one. One is defined.
  Every application a back end serves answers `kuri-link.addr` itself: an
  `open` whose
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
- **At home, if the owner chooses, it announces itself.** Sharing a network
  is not consent to be seen on it, so mDNS is off by default: a node then
  sends no mDNS packet. With `local_discovery(true)`, a back end announces its
  key and addresses on the local network (service `kuri-link`), and every node
  with it on looks peers up that way when it dials. So a device back on the
  home network finds the back end whatever its address there has become.
  Everyone on that network sees the key and addresses, though only paired
  devices get further. Front ends look up without announcing themselves.
- **What remains.** Without mDNS, a back end whose address changed is found
  again only through what `kuri-link.addr` pushed before the change, or
  through a relay. A device that is away from home, and not connected, while
  the home's public IP changes does not learn the new one until it is back on
  the home network, and then only with mDNS on.

## One gateway for a home

Without a gateway, each back end is a node of its own: its own key, its own
way in, and its own pairing, so a phone pairs once per back end. A home with
a camera service, a media server and a thermostat, each a separate program,
pairs three times and has three nodes facing outside. The gateway makes it
one of each: a phone pairs once with the home, and reaches every back end in
it.

Arrows point away from the side that dials:

```
phone ─── relay or direct ───▶ gateway ◀─── home network ─── back end "camera"
                                       ◀──────────────────── back end "media"
dials the gateway's key        the home's key,              dial out to the gateway
with ALPN kuri-link/1/<app>    pairing and authorizer       and register their apps
```

- **The gateway** runs on the home's always-on machine, holds the home's key,
  pairs the phones, and is the only node that uses a relay or a forwarded
  port. To a phone it is an ordinary back end serving every registered app:
  the phone dials its key with the app's ALPN, as it dials a back end today,
  and `kuri-link.addr` gives the gateway's own address. Nothing changes on
  the phone side.
- **Back ends dial out to the gateway and listen for nothing.** A back end,
  on any machine at home, connects to the gateway and registers the apps it
  serves; the gateway then opens a stream on that connection for each
  request it forwards. A back end therefore accepts no connection at all,
  needs no relay, mapping or mDNS of its own, and its address does not
  matter. When it stops, its connection ends, and so does its registration.
- **Back ends pair with the gateway once,** as phones do, through a second
  pairing whose door is the reserved application `gateway`. Its tickets admit
  a key to the gateway's set of back ends, never to its set of phones, and
  the reverse: a phone cannot register an app, and a back end cannot call
  one. Unpairing a back end ends its registration.
- **The home's relay carries the home alone.** A relay the home runs next to
  the gateway asks the gateway whom to carry, each time a device connects to
  it: the gateway, and the phones and back ends paired with it then. So
  pairing a phone at home is all it takes for the relay to carry it outside,
  and nobody keeps a list of keys. The relay sees only a key, not a ticket,
  so a phone not yet paired is not carried: it pairs over the home network,
  next to the code it scans.
- **An app name belongs to one back end.** The gateway records which back end
  key first registered each app. A restarted back end, same key, replaces its
  own registration; another key is refused that name until the owner unpairs
  the first. Unpairing a back end releases its names: the gateway stops
  accepting them, as it does a name no back end registered.
- **Forwarding is per request, and a whole stream.** The gateway reads the
  phone's header, authorizes the request, and opens a stream to the back end
  with the same header plus the app, the phone's key and the authorizer's
  claims. It then copies both directions until both finish, so `call` and
  `open` work unchanged. QUIC flow control on both legs bounds what the
  gateway buffers.
- **Authorization happens at the gateway.** Its authorizer sees the phone's
  key proven by the handshake, the app, the method and the credential, as a
  back end's does today; by default it admits the paired phones. Unpairing a
  phone there refuses it everywhere from its next request. The gateway is
  the one place where access is decided: an authorizer added on top, such as
  one that checks grants, goes on the gateway, and the back ends do not check
  grants again.
- **A back end trusts the gateway's word.** Its `Caller` is the phone, as the
  gateway vouches, with the gateway's claims, so a service sees which phone
  calls and needs no change. Its own authorizer may narrow further; by
  default it admits what the gateway admitted. It takes requests only over
  its own connection to the gateway it paired with, whose key the handshake
  proved, and sees no proof of the phone's key: a gateway that is
  compromised can act as any phone towards every back end. That is the price
  of one door, and why nothing but the gateway holds the home's key.
- **A missing back end is a fault, not a refused connection.** The gateway
  keeps accepting an app's ALPN while its back end is down, and answers each
  request with a new fault code, `unavailable`, which a front end from
  before it reads as `unknown`. Waking moves to the gateway too: before
  answering `unavailable`, it can call a `Waker` for the back end and wait
  for it to register. (Waking at the gateway is not built yet.)
- **A back end gone without a word is noticed soon.** A back end that
  crashes, or loses power, sends no goodbye, and requests forwarded to it wait
  until its connection times out. A back end therefore sets that timeout to
  4 seconds, instead of the 30 that suit a phone on a weak network, and sends
  a keep-alive every second, so that a quiet connection stays up; the shorter
  of the two ends' timeouts applies. When it registers again on a new
  connection, as after a restart, the gateway closes the old one at once, and
  requests still on it fail instead of waiting.
- **So is a gateway that crashed.** A gateway started in place of one that
  crashed does not know the old connections, and until a back end registers
  again, the gateway answers requests for its apps `unavailable`. The same
  timeout tells the back end: it registers again 5 to 6 seconds after the
  crash.
- **These timers suit a back end on the gateway's network,** the usual case
  at home. One that reaches the gateway through a relay, or over WiFi that
  drops out, registers again whenever nothing passes for 3 to 4 seconds, and
  its apps answer `unavailable` until it has. A registration that held less
  than 30 seconds counts as a failure, as when two back ends share a key, so
  on such a path the waits between its tries can grow to 30 seconds.

Wire additions, all within version 1:

```
register: B→G on ALPN kuri-link/1/gateway
          Header{v:1, op:"open", method:"kuri-link.register", args:{"apps": [name, …]}}
          G→B Reply, then frames as for kuri-link.addr: the gateway's
          address, then each change. The apps stay registered while this
          stream is open.
forward:  G→B a stream the gateway opens on the same connection:
          Header{…the phone's header…, "app": name, "caller": bytes(32),
                 "claims"?: value}
          then as for call or open
code:     unavailable (no back end serves the app now)
```

In the API, a `Gateway` holds the back ends' pairing and who owns each app
name, kept in a file with `Gateway::open`; `NodeBuilder::gateway` makes a node
serve it. A back end is built with `NodeBuilder::behind_gateway(addr)`, pairs
once with `Node::pair_gateway(ticket)`, and keeps the gateway's address as
`Node::gateway_addr` reports it. `Node::registration` says whether the
gateway forwards to it now, or refused it and why.

Choices made, and why:

- **Pairing, not discovery, connects a back end to the gateway.** Registering
  an app hands a program every phone's requests for it. That is the owner's
  decision, not something a program on the home network gets by finding the
  gateway. mDNS, when the owner turns it on, only finds a gateway whose
  address changed.
- **No way around the gateway, at home or away, in this version.** If the
  gateway is down, every app is. A second way in, phones dialling back ends
  directly at home, would need each back end to listen and to know the paired
  phones, and an unpairing would have to reach every back end: two doors, and
  two locks to keep in step. The gateway runs on the machine that is always
  on anyway, its supervisor restarts it, and a phone reports that the home
  gateway is not answering rather than a generic failure.

  The direction after that: at home, heavy traffic such as a camera's video
  or a media stream goes straight between the phone and the back end, and
  the gateway only introduces them and authorizes. It decides who may talk; the
  bytes do not pass through it, and connections it set up keep working while
  it restarts. Nothing in version 1 closes that off. The credential in a
  header is opaque, so a phone can carry one the gateway issued, which a back
  end checks against the gateway's key without asking it; a method under the
  reserved `kuri-link.` prefix can hand such a credential out; and a field a
  peer does not know is ignored, so the header can grow.
- **The gateway is a role of the library, run by a small program of its
  own.** `kuri-link` gets a gateway builder, and a standalone program hosts
  it by default. An always-on hub may embed the same code, but by default it
  is a process of its own, so that restarting or updating an app, its UI
  included, never cuts the phones off.
- **Not end to end.** A gateway that only introduced phones to back ends, and
  passed on their encrypted traffic, would stay out of the plaintext. But
  every back end would then face outside, and every one would have to hear of
  each pairing and unpairing. One door is the point.

A gateway whose address changed needs no new pairing. A back end knows the
gateway by its key, and dials the address it last heard and the relays in it:
a ticket the gateway issues names its relays, and a back end dials through
them even when it uses no relay of its own. With a relay, a back end that was
off while the gateway moved finds it wherever it went; mDNS only finds it
sooner at home. What remains: with neither a relay nor mDNS, that back end
does not find the gateway until its address is given again.

## Hooks

| Hook | Default | Purpose |
|---|---|---|
| key (`secret_key`, `load_or_create_key`) | ephemeral key | the node's identity |
| `Authorizer` | `DenyAll`, or the paired set with pairing on | who may call what |
| `Pairing` | off | let devices join by one-time ticket |
| `Relays` | `None` | which relays may carry traffic |
| `port` | random | the port a back end keeps, so its public address survives restarts |
| `port_mapping` | off | ask the home router to forward the port: an inbound port into the home |
| `local_discovery` | off | announce and look up nodes on the local network (mDNS): seen by everyone on it |
| `Waker` | none | wake a sleeping back end before one retry |
| `gateway` | off | be the home's gateway: pair phones once, forward to the back ends behind it |
| `behind_gateway` | off | serve through a gateway: dial out to it and listen for nothing |

## Not yet

- Waking a back end from the gateway. Today a phone hears `unavailable`
  until the back end registers again.

- Typed Dart clients generated from Rust service definitions. Today the Dart
  side calls by method name with CBOR bytes.
- A server-streaming helper. Today it is built on `open`.
- Browser front ends. These could only ever go through a relay, because browsers
  cannot open raw UDP.
