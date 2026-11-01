# Roadmap

What is merely *designed* belongs in the architecture docs; what is *planned*,
or under consideration, belongs here. A defect that has been fixed needs no entry anywhere — the commit
that fixed it, and the test that holds it fixed, are the record. The limitations
imposed on us from outside are recorded beside the mechanism they constrain, which
is the only place they can be read in context.

## Planned

### An official way to have Windows rate-control its H.264 over TCP

An adaptive session has the host rate-control its H.264 by taking the network
detection and claiming reliable UDP in `CS_MULTITRANSPORT`, then turning the
host's offer of UDP down (`E_ABORT`) and staying on TCP
([The connection sequence](rdp-client.md#the-connection-sequence)). It works
against the hosts measured, but it depends on host behaviour the specification
does not promise: nothing in MS-RDPBCGR or MS-RDPEMT says a claim of UDP that is
then refused changes how the host codes for TCP, and a Windows update may stop
doing it. What is to be found out is whether there is a cleaner or official way
to the same rate control:

- What Microsoft's clients actually send: mstsc, the iOS and macOS apps, and
  the web client over its gateway, read through a recording proxy — whether
  they claim UDP with UDP turned off, which connection type and detection
  results they state, and which graphics capability sets and flags they
  advertise.
- Whether a capability, a connection type or a detection result alone does it,
  without the claim of UDP.
- Whether a group policy or registry setting on the host does it, for an
  operator who can set one.
- What the RDP 10.x graphics capability sets (MS-RDPEGFX) and the MS-RDPBCGR and
  MS-RDPEMT text on network detection and multitransport say of it, read from
  the specifications rather than recalled.

### A second display's tab that says when the session's page is gone

On `rdp`, `ard-high-performance` and `wlshare` targets with two displays
([Windows](https://zdesk.dev/guides/zdesk#windows)), when the session's page goes — closed,
reloading, or its socket dropped — the tab showing the second display carries on
with no word of it: its input still reaches the remote through the reattach
grace, and only the engine ending takes the display away. The tab's display
socket already carries one thing that is the session's rather than the
display's, the clipboard ([Clipboard](architecture.md#clipboard)). The plan is
for it to carry whether the session's page is attached too, and for the tab to
say so over its desktop until the page is back.

### A touchpad's scroll as a gesture on a Mac

A Mac is sent a scroll as a distance on both axes, with no gesture around it
([a Mac scrolls by a distance](apple-vnc-889.md#a-mac-scrolls-by-a-distance)).
Horizontal scrolling from a touchpad feels less fluid that way than vertical. A
two-finger swipe is rarely straight: on a Windows precision touchpad the browser
reports both axes in most swipes, as finely on one as on the other, and a Mac
holds a scroll to the axis it began on only within a gesture.

The plan is to send the gesture. The scroll message has a field for the scroll
phase and one for the momentum phase, which Apple's viewer fills from its own
trackpad. A browser's wheel events carry neither, so the gateway would open a
gesture at the first event of a glide, continue it while events keep coming and
end it once they stop. What the Mac does with a gesture it is sent that way, and
whether the phases alone make the horizontal axis feel right, are still to be
tested, on a physical Mac.

## Under consideration

### Verifying an RDP host's certificate

The TLS handshake accepts any certificate, for the session only, and verifies the
handshake signature alone (`proto/tls.rs`). What stands in for the chain is
CredSSP: the credential exchange is bound to the public key of the certificate
that terminated this very handshake, so an interceptor holding a certificate of
its own cannot complete it. That argument is spelled out in the module's header
and holds only because plain TLS is never offered.

What it does not give an operator is a way to say *which* host they expect. A
Windows host's default listener certificate is self-signed and regenerated when
the machine is renamed, so a public CA chain is the wrong check for most targets;
a per-target pin — the certificate's public key or its SHA-256 fingerprint in the
target's configuration, compared against what the handshake presented — is the
one that fits. Absent a pin, today's behavior stands.

### Licensing on a Remote Desktop Session Host

The licensing step accepts exactly one PDU: an `ERROR_ALERT` carrying
`STATUS_VALID_CLIENT`, which is what every Windows host this client has been
pointed at sends. Anything else is refused by name in
`proto/license.rs`, and the connection ends before `DemandActive`.

A Session Host with the Remote Desktop Session Host role and per-device CAL
licensing does not send that. It opens a real exchange: `LICENSE_REQUEST`, the
client's new or upgrade licence request, the platform challenge and its response,
and the issued licence. This client would disconnect at the first of those.
FreeRDP's `libfreerdp/core/license.c` is an implementation to compare while
writing the exchange; a temporary reference checkout belongs under
`tmp/references/`.

How many real deployments this reaches is not known, and no host in use has shown
it. What would settle it is one connection to an RDSH configured for per-device
CALs; the work is only worth taking once a host that needs it turns up.

### The pipeline compressed again for the browser

Taken up if a pipeline's bytes to the browser come to matter, a Lossless host
over anything short of a LAN first. A passed pipeline reaches the browser in the
host's own bulk compression
([RDP's graphics pipeline, passed through](architecture.md#rdps-graphics-pipeline-passed-through)),
whose history is 2.5 MB. Unwrapped and compressed again with zstd at level 1,
with a 32 MB window and long-distance matching, captures of a Windows 11 host
under the Lossless policy at 3456×1802 (2026-10-15) came out smaller than the
host's compression on every one: by 21% on a still desktop's opening paint, 25%
on a desktop busy about half the time, 19% with a video in a window, and 66% on
scrolling, where a scrolled page is mostly rows the host sent a frame or two
earlier, megabytes back. Compressing ran at 280 to 580 MB/s on one core,
against at most 25 MB/s from the host. The cost is another C library in every
gateway build and in its packaging, a zstd decoder in the page's WebAssembly
module, and 32 MB of window in the page per pipeline. A 64 MB window gained only
on scrolling, from 6.15 to 7.36 times smaller than
unwrapped. The record and its
acknowledgement would stay as they are: one stream per pipeline, started at
`graphicsStart` and flushed at each record, in place of the host's.

### A settings file for the TUI

Taken up when the TUI needs a setting that belongs to the control plane rather
than to any one instance, such as a locale for its screens. Nothing needs one
today: the only such setting is the shared port, and `--port` and
`ZDESK_TUI_PORT` cover it.

The file would be `tui.toml` in the instances directory, beside the instance
folders it applies to, so a control plane started with `--instances-dir` has its
own. The first start would write it with every key commented out, so the
defaults hold and the file shows what can be set. It would be parsed strictly:
an unknown key or a bad value refuses the start instead of being skipped. The
port would move into it as `port`, with `--port` and `ZDESK_TUI_PORT` still
taking precedence. A change would apply at the next start.

### A passkey as the login

It builds on the TUI's PIN login
([Local multi-instance control plane](architecture.md#local-multi-instance-control-plane)).
The user registers a passkey once and presents it instead of a PIN or a
password. A served gateway needs it most, since its login is the one reached
from outside the machine. It would still start on the TUI, which has the
owner-only instances directory to keep the public keys in. The TUI is also the
only place with the one-ID-over-many-subdomains layout. A failed passkey is
recoverable there, because its PIN is still at the terminal.

One passkey covers every instance because of how WebAuthn scopes a credential,
not through any wildcard. A credential is bound to a relying-party ID: a bare
domain with no scheme or port. A page may use an ID equal to its own host, or
to a parent of that host that is not a public suffix. The ID `zdesk.localhost`
therefore works from the landing page and from every
`<instance>.zdesk.localhost`, provided both ceremonies name it explicitly:
`rp.id` in `navigator.credentials.create()` and `rpId` in
`navigator.credentials.get()`. Left out, either one defaults to the page's own
host, so a passkey registered on a subdomain is scoped to that subdomain alone,
and a login from a subdomain finds none of the ones registered on
`zdesk.localhost`. Plain `http` is enough on loopback in Chrome and Safari,
which treat `localhost` and its subdomains as a secure context. Other browsers
have not been checked, and one that does not do the same needs HTTPS for
WebAuthn. This was checked on 2026-10-27 with a small relying party on the
shared port. A passkey registered on `zdesk.localhost` logged in from two
levels of subdomain in Chrome and Safari. Chromium's virtual authenticator
(`WebAuthn.addVirtualAuthenticator` over CDP) drives the same ceremonies
headless, so a Playwright spec can hold the behaviour under the usual rules.

The master would serve the registration and login ceremonies wherever the PIN
is entered, naming `zdesk.localhost` as above, and its login cookie would carry
the result to every origin as the PIN's does. It would verify each ceremony itself before it
keeps a credential or logs a browser in, checking:

- `type` in the client data: `webauthn.create` for a registration and
  `webauthn.get` for a login.
- The challenge: one the master issued for this ceremony, at least 128 random
  bits, expiring after a few minutes, and consumed by its first use, whether
  that use succeeded or not.
- An origin of `http://zdesk.localhost:<port>`, and the ID's hash in the
  authenticator data.
- The user-present flag on every ceremony, and the user-verified flag too,
  with `userVerification: "required"` asked for. A passkey stands in for the
  PIN, so the authenticator must check its holder.
- On a registration, a discoverable credential: the master asks for one with
  `authenticatorSelection.residentKey: "required"`, so the passkey can be found
  without a username. It also asks for the `credProps` extension and refuses a
  registration whose `rk` is not `true`. The browser reports that, and nothing
  signs it. A login asks with an empty `allowCredentials`, so the authenticator
  offers every passkey it holds for the ID and returns its user handle.
- On a login, a credential ID the stored file holds and a user handle equal to
  the one it was registered with. The signature is then checked against that
  credential's stored public key.

The public keys would live in an owner-only file beside the instance folders,
so a control plane started with `--instances-dir` has its own. Only a browser
already logged in with the PIN could register a passkey. The TUI would list the
registered passkeys, with a key that removes one. Which of these settings belong
in the file described in
[A settings file for the TUI](#a-settings-file-for-the-tui), if any, is decided
when both exist.

A served gateway would follow the TUI. Its ID would be the operator's host
name. WebAuthn requires a secure context, and the HTTPS that the gateway
already needs from its reverse proxy provides one
([The client](https://zdesk.dev/guides/zdesk#the-client)). Two questions come first: where a gateway
running in a container keeps the public keys, and how an operator locked out by
a failed passkey gets back in from outside the machine. Its password login
stays as the way to register the first passkey, and as that way back in, until
something better exists.

### Touch on an RDP target (MS-RDPEI)

Taken up if the need for it shows. Nothing carries touch today: the RDP client
does not ask for MS-RDPEI, and the page treats fingers as a trackpad alone. The
touchscreen path an earlier engine had — a browser layer forwarding contacts, a
wire message for them and one announcing the channel — was removed with it, so
it would be written again on both sides. It would have no config key, since
whether touch exists is the host's answer.

MS-RDPEI is a *dynamic* channel and that transport is already here:
`proto/dvc.rs` carries Display Control over `drdynvc`, and the session answers
every Create Request it does not want with `NO_LISTENER` (`session.rs`).
Accepting a second name, the RDPEI PDUs — client ready, and a touch event's
contact frames — and the contact state machine are the client's part.

Above the channel: a message telling the page the host opened it, re-sent to
each client that attaches; a contact message — down, move, up, cancel, in
framebuffer pixels, named by small slot ids — that the session follows like a
held key, so contacts still down are cancelled when a client goes away, or the
remote keeps fingers down that no longer exist; and a page layer, offered only
after the announcement, that forwards fingers uninterpreted in place of the
trackpad gestures and adds no gesture recognition of its own: the guest
recognises them. A Windows host opens MS-RDPEI and xrdp never does, which is
the reason this would be an always-offered capability rather than a key.

### zdesk-relay beyond QA

[`zdesk-relay`](../crates/zdesk-relay/README.md) reaches a High Performance Mac
the gateway cannot reach itself. As it stands it is for QA: it authenticates
nobody, and it carries the Mac's media stream to the gateway over TCP. That
was checked on a LAN only, against a virtual Mac behind its host's NAT: 6 ms of
one-way delay and nothing dropped. Two things would take it past QA:

- **Authentication.** Something the gateway proves before the relay connects
  it anywhere, beyond the `--allow` networks that are the only limit today.
- **The media stream over UDP.** Over TCP, one lost segment holds back every
  datagram behind it until it is sent again, and a link slower than the Mac's
  20 Mbit/s floor ([Rate control](apple-vnc-889.md#rate-control)) queues the
  excess instead of losing it, so the picture falls seconds behind. The Mac
  also sets its rate from the delay it sees, and TCP turns the link's losses
  into queueing delay first. Over UDP the excess is lost, the gateway asks for
  a keyframe, and the picture stays current.

  The plan is to keep the session on TCP and have the relay also listen on UDP
  at the same port number. The gateway would send the media there from one
  socket, each datagram prefixed by its session and port, and the relay would
  learn the gateway's address from the first one. Since the gateway sends
  first, its firewall or NAT lets the relay's datagrams back in, and its
  reports every second keep that open, so nothing has to be opened towards the
  gateway. The framed TCP media connection would stay as the route for a
  network that blocks outbound UDP. Only the media connection's two ends
  change, since the receiver reads every port through one type
  (`Datagrams` in src/vnc_apple_media.rs).

  QUIC on one UDP port, the session as a stream and the media as datagrams,
  would bring encryption and a place for the authentication. It would cost a
  QUIC library and certificates, and the RFB code reads a `TcpStream` directly.

## Not planned

### Decoding an RDP host's H.264 at the gateway

The gateway composes no graphics pipeline: every pipeline is passed to the
page, and a High quality session refuses H.264 with `AVC_DISABLED`
([RDP's graphics pipeline, passed through](architecture.md#rdps-graphics-pipeline-passed-through)).
Decoding H.264 here only to send it as tiles would be a second lossy pass, of a
stream the page already decodes.

### Encodings other than ZRLE on a plain VNC target

A plain `vnc` target advertises ZRLE, and Raw beside it: CopyRect, zlib, Hextile
and RRE are for servers without ZRLE, Tight and TightPNG are vendor encodings,
JPEG and H.264 are lossy, and advertising an encoding is a promise to decode
it. Decoding the Tight family, or handing a
lossy payload to the browser untouched, would remove upstream bytes and a
transcode at the cost of a decoder this repo would then own. That work is for a
server outside the three the project prioritizes, which is reached through the
RFB baseline and nothing more
([Constraints](architecture.md#constraints)). A stream of its own passed
through is what a `wlshare` target has.

The gateway does decode zlib, since a Standard Mac's Full quality is RFB's zlib
asked for ahead of ZRLE as Apple's viewer asks
([Full quality's zlib](apple-vnc-889.md#full-qualitys-zlib)); the decoder sits
beside ZRLE's and Raw's (`vnc_encodings`, `Payload::Zlib`). Listing it on a plain
target would cost only the entry and a test, and is still not planned: every
VNC server in use has ZRLE and prefers it when a client lists it first, so the
listing would be read by no server this project meets, and would be a path the
end-to-end tests against a real server never exercise.

### More than two virtual displays on a target

`virtual_displays` is held to two. A browser tab shows one display: the one on
the session's page, and on *All Displays* (beta) the second in a tab of its own
at `/display/2`. Nothing of either is encoded here: a Mac's streams are passed
only while their display is shown, and an RDP host's span is passed whole and
composed once in the page. Each
further display is a desktop the host renders for nobody but a further tab, and
two is the most the picker, the input offset, the tab's feed and the span the host
builds have been checked with. RDP would allow sixteen in a row (MS-RDPBCGR 2.2.1.3.6). A Mac's High
Performance mode advertises how many virtual displays it will create, which is
two, holds a configuration naming more to two, and has a video leg for a second
display and no third, so two is the limit there whatever this gateway asked for.
What holds the ceiling is that nothing shows more: a person at
a browser looks at one screen, and a third display that is never on it is work
the host does for a menu entry. Raising it means first wanting the view that
would use it, such as the Mac's All Displays composed across virtual displays,
which is not planned either.

### Multiple sessions

Concurrent sessions, shared sessions and a session broker are not planned. One
gateway instance has one active session slot, as defined by the
[session constraints](architecture.md#sessions). This keeps engine ownership,
input state and per-session media choices attached to one browser claim.
