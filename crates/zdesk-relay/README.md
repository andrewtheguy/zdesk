# zdesk-relay

> **For QA only.** The relay has no authentication yet: anyone who reaches
> its port can connect through it to every address its `--allow` covers. Run
> it only on a network you trust, for as long as a test needs it.

A relay for reaching a Mac that the gateway cannot reach itself, such as a
virtual Mac behind its host's NAT. It runs on a machine that reaches the Mac,
such as the host of the virtual Mac.

High Performance Screen Sharing needs more than a TCP port forward. The Mac sends
its media stream over UDP, from its own address to the viewer's address on the
TCP connection, using the same port numbers at both ends (5900, 5901, and 5902
for a second display). See
[Reaching the gateway](../../docs/apple-vnc-889.md#reaching-the-gateway).

The relay connects to the Mac in the gateway's place, so the Mac sends the
stream to the relay. The relay binds those ports on its own address towards the
Mac and carries their datagrams to the gateway over TCP.

The gateway needs only outbound TCP to one port on the relay, and no port is
opened towards it. It makes two connections there: one for the session, and one
for the media stream's datagrams, which go in frames.

## Run it

Build it on a Mac, or on any machine that will run it:

```sh
cargo build --release -p zdesk-relay
```

Start it on the machine that reaches the Mac. Pass `--allow` once for each
network it may connect to:

```sh
zdesk-relay --listen '[::]:15900' --allow 192.168.160.0/24
```

The relay logs each connection. When a media connection ends, it logs how many
datagrams each port carried in each direction, which shows whether the Mac's
stream reached the relay. `RUST_LOG=debug` adds more.

## Point a target at it

On an `ard-high-performance` target, `relay` names the relay. `host` is the Mac
as the relay reaches it, not as the gateway would:

```toml
[[targets]]
name = "macvm-relayed"
protocol = "vnc"
subtype = "ard-high-performance"
host = "192.168.160.2"
relay = "[fdb8::1]:15900"
username = "andrew"
password_ref = "mac"
```

## The wire

Each connection starts with one line from the gateway, and the relay answers
with one line:

- `zdesk-relay/1 connect <host:port>` gets `ok <token>`, after which the
  connection carries the Mac's TCP bytes.
- `zdesk-relay/1 media <token>` gets `ok`, after which the connection carries
  frames. A frame is a kind byte, a big-endian port and a big-endian length,
  followed by that many bytes of payload. Kind 0 opens a port, and only the
  gateway sends it. Kind 1 is one datagram of that port, in either direction.

Every refusal is `error <reason>`, after which the relay closes the connection.
The media stream crosses the relay on TCP, so a lost segment delays the
datagrams behind it rather than losing one. That is acceptable on the LANs QA
runs on.
