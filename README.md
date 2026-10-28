# zDesk

English | [简体中文](README.zh-CN.md) | [繁體中文](README.zh-TW.md)

Remote desktop in a browser, for Windows' and macOS's own remote desktop
servers, and for Linux through wlshare. zDesk is a small gateway you run next
to your machines. It connects to:

- **Windows 10 and 11's own Remote Desktop**, over RDP with its modern graphics
  pipeline. Windows Home cannot host it.
- **macOS's built-in Screen Sharing**, in both of its modes: Standard and High
  Performance.
- **Linux**, through [wlshare](https://github.com/andrewtheguy/wlshare), our own
  VNC server for wlroots compositors.
- Other RDP and VNC servers, through each protocol's baseline.

Windows and macOS need nothing installed on the host, only their own sharing
switched on. The gateway serves one web page that shows the desktop, so anything
with a browser, on Windows, macOS, Linux, iOS or Android, reaches every one of
them with no client to install either. Your desktops are not tied to one device
or place: wherever a browser can reach the gateway, you have your machines.

```text
browser ── HTTPS or localhost ── zDesk ── RDP / VNC ── your desktops
```

`zdesk serve` is single-user: one login, a list of your targets, and one
active session. Native installs also include [`zdesk tui`](docs/guide.md#local-instances),
a local control plane that runs several independent gateway instances side by
side.

> [!WARNING]
> **No backward compatibility.** zDesk is still settling its
> [direction](docs/architecture.md#direction), and a release may change or
> remove a configuration key, the WebSocket protocol or a feature, with nothing
> that reads the old form. Check [`zdesk.example.toml`](zdesk.example.toml) against your config
> when you upgrade. The page and the gateway are one release too: a tab left
> open across an upgrade names both versions and asks to be reloaded.
> This is about compatibility, not stability: I have been actively using the
> features not marked experimental, and they have been stable.

## Demo

A Mac's Screen Sharing in a Chrome tab on Windows:

https://github.com/user-attachments/assets/fd0610bb-444a-4222-aa1d-76a39d09b3f0

Watch more demos on the [zDesk site](https://zdesk.dev/demo).

## Features

- **One browser client, nothing to install.** Chrome, Edge or Safari on a
  desktop, Chrome on Android, and Safari on an iPhone or iPad. On a desktop,
  install the page as an app from Chrome's or Edge's menu to give it a window
  of its own.
- **Sized to the browser, sharp at its density.** Windows, a High Performance
  Mac and wlshare's headless outputs follow the window at its screen density,
  so a Retina or HiDPI display shows them sharp and at their true size. A
  Standard Mac's displays keep the density they report.
- **Two displays at once (beta)** on Windows, a High Performance Mac and
  wlshare, the second in a browser tab of its own. Apple's own viewer on
  macOS 26 offers no dynamic resolution with two virtual displays.
- **The remote's own picture, decoded in the browser.** Windows' graphics
  pipeline, a High Performance Mac's HEVC or H.264, a Standard Mac's Adaptive
  picture and wlshare's VP9 are passed through untouched, and all but the High
  Performance Mac's are paced to the browser link.
- **Tiles that settle to lossless.** Where the gateway codes the picture itself,
  it sends 64×64 tiles: PNG where they are exact, JPEG where they move, and
  sent again exactly once they are still.
- **A gateway with a low memory footprint.** One Rust binary serves one session
  at a time, and where the remote's picture and sound are passed through, the
  browser decodes them, so the gateway holds no decoder or decoded picture for
  them.
- **Remote sound in the browser**, as Opus paced to the browser link, lossless
  FLAC for a LAN, or a High Performance Mac's own AAC-ELD.
- **Clipboard text in both directions**, synced automatically where the browser
  permits it and through the Clipboard panel where it does not.
- **Camera and microphone redirection (experimental)** for an RDP or wlshare
  session started with them at the picker.
- **Reattach without losing the desktop.** After a reload or a dropped socket,
  the owning browser has 60 seconds to reattach. A takeover or later connection
  starts a fresh session, but Windows, a Mac and wlshare keep the desktop
  running, with its windows where they were left.
- **Mouse, keyboard and touch.** Installed as an app, the page receives the key
  chords a tab leaves to the browser. On a phone or tablet, fingers act as a
  trackpad, with pinch zoom and a soft keyboard.

## Supported servers

zDesk is built around the remote desktop servers native to each platform:
Windows' own Remote Desktop, macOS's built-in Screen Sharing, and on Linux
[wlshare](https://github.com/andrewtheguy/wlshare), our own. Windows and macOS
come first: they are what most people connect to, so features are tested and
optimized on them first. wlshare follows them, though it integrates the most
seamlessly of the three, since what RFB lacks is added to it.

- **Windows 10 and 11's Remote Desktop** (`rdp`). Follows the window at its
  density and carries Opus or FLAC sound, with the camera and microphone beside
  it. Its picture is its graphics pipeline, passed through, high quality or, in
  alpha, adaptive H.264.
- **A Mac's High Performance Screen Sharing** (`vnc`,
  `subtype = "ard-high-performance"`). Follows the window at its density, on
  one virtual display or two, and always carries AAC-ELD sound. Its picture is
  its HEVC or its H.264 (beta) passed through, which the Mac sends at its own
  rate rather than the browser link's: little on a still screen, much with motion.
  The H.264 needs a browser with `VideoDecoder` but nothing added to the page;
  the HEVC needs a browser that decodes it or the page's software decoder.
- **A Mac's Standard Screen Sharing** (`vnc`, `subtype = "ard"`). The Mac's
  physical displays, one or all, at their own size and density, as the
  gateway's tiles of the Mac's lossless Full quality or as the Mac's own Adaptive
  picture passed through (beta), with no sound, camera or microphone.
- **wlshare on Linux** (`vnc`, `subtype = "wlshare"`). Because the server is
  ours, what RFB lacks is added to it: it switches outputs, follows the window
  at its density, carries Opus or FLAC sound and the browser's camera and
  microphone, and codes its own VP9 for the gateway to pass through, adapting
  to the browser's link — or, for a link with room to spare, its own exact
  tiles with a video stream for each region that moves, H.264 from hardware or
  libx264. Two outputs can be shown at once, the second
  in a browser tab of its own (beta).

Every other VNC server is a plain `vnc` target reached through the RFB
baseline: one framebuffer at 1x, not resized, no sound, tiles from the gateway.
Another RDP server may happen to work but is not tested against. See
[Prioritized servers](docs/architecture.md#prioritized-servers) for the
comparison table, what they share, and what each passes.

## Server and browser support

What each server and browser does and does not do, in one place.

| | Windows Remote Desktop | Mac, High Performance | Mac, Standard | wlshare | Other VNC |
|---|---|---|---|---|---|
| Resizes to the browser window | ✅ with the graphics pipeline (default) | ✅ | ❌ physical displays at their own size | ✅ headless outputs only | ❌ the server's size |
| Sharp at HiDPI/Retina density | ✅ | ✅ | ✅ at the density the Mac reports | ✅ | ❌ 1x only |
| Second display in its own tab | ⚠️ beta, with the graphics pipeline | ⚠️ beta | ❌ one screen at a time, switched from the display menu, or every screen combined in one tab (alpha) | ⚠️ beta, with two outputs | ❌ |
| Sound | ✅ Opus or FLAC | ✅ AAC-ELD, always | ❌ | ✅ Opus or FLAC | ❌ |
| Camera and microphone | ⚠️ experimental | ❌ | ❌ | ⚠️ experimental | ❌ |
| Unicode clipboard | ✅ | ✅ | ✅ | ✅ | ⚠️ with Extended Clipboard, else Latin-1 |
| Picture paced to the browser link | ✅ | ❌ the Mac's own rate | ✅ | ✅ | ✅ |
| Picture settles to lossless | ⚠️ only when the host's policy asks | ❌ | ✅ with Full quality | ⚠️ with *Tiles with motion* | ✅ |

| | Chrome, Edge | Safari | Firefox |
|---|---|---|---|
| A High Performance Mac's HEVC | ✅ native; on Windows only where the GPU decodes HEVC 4:4:4, else hevc-wasm | ✅ native | ⚠️ hevc-wasm only |
| A High Performance Mac's sound (AAC-ELD) | ✅ | ✅ except 16.4 to 18, which play no sound at all | ❌ |
| Lossless FLAC sound | ✅ | ❌ Opus only | ✅ |
| Key chords the browser keeps for itself (⌘W, ⌘T, ⌘N) | ✅ in an app window or Immersive full screen | ❌ | ❌ |

hevc-wasm is the page's software HEVC decoder, served by a gateway that finds
its release archive in its install's `share/zdesk`, or where `[hevc_wasm]`
names one kept elsewhere. Where neither it nor the browser decodes the HEVC, the
picker offers the Mac's H.264 (beta) if the browser has `VideoDecoder`; without it,
neither picture is offered and the target cannot start.

An app window is the page installed from the browser's menu: in Chrome, ⋮ →
*Cast, save and share* → *Install page as app*; in Edge, ⋯ → *Apps* → *Install
this site as an app*. It opens in a window of its own, without tabs or an address
bar, and receives ⌘W, ⌘T, ⌘N and the like. *Menu → Immersive full screen* in a
tab does the same and adds Alt+Tab and the Super key.

- **A secure context is required:** HTTPS, or `localhost` directly or through an
  SSH tunnel. A LAN address over plain HTTP is refused.
- **WebCodecs carries video streams and sound.** Without it, only the gateway's
  tiles, a Standard Mac's Adaptive picture and an RDP pipeline without H.264
  work, with no sound.
- **A High Performance Mac's picture does not adapt to the browser link:** a
  still screen sends little, motion much.
- **One active session per gateway.** A takeover ends it and starts at the
  picker.
- **Touch is a trackpad:** fingers are not sent to the remote as touchscreen
  contacts.
- **Other RDP servers** may work but are not tested against.

## Quick start

Releases keep no backward compatibility (see the warning above), so install a
specific version and stay on it until you choose to upgrade. Pick one from the
[releases](https://github.com/andrewtheguy/zdesk/releases) and put its number in place of `0.0.x`.
On Debian 13 or Ubuntu 24.04 and later:

```sh
curl -fsSLO https://github.com/andrewtheguy/zdesk/releases/download/v0.0.x/zdesk-linux-amd64.deb
sudo apt install ./zdesk-linux-amd64.deb
```

The package then runs either a served gateway, `zdesk serve`, from a config
you write, or the control plane, `zdesk tui`, a terminal interface that runs
gateways on the machine you use. The two sections below set up each.

### Served gateway: `zdesk serve`

Create the config from the shipped example and generate a login:

```sh
sudo install -d -m 700 -o "$(id -un)" -g "$(id -gn)" /etc/zdesk /var/lib/zdesk
sudo install -m 600 -o "$(id -un)" -g "$(id -gn)" \
  /usr/share/doc/zdesk/zdesk.example.toml /etc/zdesk/zdesk.toml
zdesk gen-passwd admin
```

Put the generated value and a target in `/etc/zdesk/zdesk.toml`, with the
file the gateway reads its master password from:

```toml
[server]
site_passwd = "admin:$2b$..."
master_password_file = "master-password"

[[targets]]
name = "workstation"
protocol = "rdp"
host = "192.0.2.10"
username = "Administrator"
password_ref = "workstation"
```

Then paste in the target's password, encrypted under your master password (see
[Target passwords](docs/guide.md#target-passwords)): `password master create`
asks for a new master password, writes it to that file, readable by you alone, and
prints a `[master]` block for the config; `password encrypt` prints the value
to add as `workstation = "…"` under `[passwords]`.

```sh
zdesk password master create
zdesk password encrypt
```

Start it and open <http://localhost:52380>:

```sh
zdesk serve
```

The page needs a secure context, so from another machine put the gateway behind
a TLS-terminating reverse proxy; `localhost` works as it is.

There are also `.rpm`, macOS `.pkg` and Windows `.msi` packages, and a container
image. Its config sets
`[server].master_password_file = "/run/secrets/zdesk_master"` and the master
password is mounted there; [Container](docs/install.md#container) shows how to
create that file and print the config's `[master]` and `[passwords]` with the
image:

```sh
docker run -d --name zdesk -p 52380:52380 \
  -e ZDESK_LISTEN=0.0.0.0:52380 \
  -v ./zdesk.toml:/opt/zdesk/etc/zdesk.toml:ro \
  -v ./secrets:/run/secrets:ro \
  ghcr.io/andrewtheguy/zdesk:v0.0.x
```

### Control plane: `zdesk tui`

On a desktop with a browser and a keychain on the same machine (macOS, Windows,
or Linux with GNOME Keyring or KWallet), the native packages' control plane
runs gateways instead, each with its own config, and none of the steps above
are needed:

```sh
zdesk tui
```

It listens on loopback only, at <http://zdesk.localhost:52380>. Press `n` to
create an instance, `e` to add a target to its config, `p` to encrypt the
target's password, `s` to start the instance and `o` to open it in the browser.
[Local instances](docs/guide.md#local-instances) lists every key. The container
image has no control plane.

[Installing zDesk](docs/install.md) covers every platform, upgrades and
removal.

## Documentation

- [Using zDesk](docs/guide.md): the browser client, starting a session, setting up Windows, Mac, wlshare and VNC targets, `zdesk tui`
- [Installing zDesk](docs/install.md): packages, first configuration, upgrade, removal
- [`zdesk.example.toml`](zdesk.example.toml): every configuration key, annotated
- [Known issues](docs/known-issues.md): faults worth recognising rather than re-investigating
- [Roadmap](docs/roadmap.md): what is planned, and what is not
- [Architecture](docs/architecture.md): the design and its constraints, the client protocol, the engines
- [The RDP client](docs/rdp-client.md), [Apple RFB 003.889](docs/apple-vnc-889.md): each protocol as implemented and measured
- [Development](docs/development.md): running from a checkout, checks, end-to-end tests, builds
- [Packaging](packaging/README.md): package layouts, build scripts, releases

## Development

You need Rust with rustup and [Bun](https://bun.sh).

```sh
bun install --cwd frontend
cp zdesk.example.toml zdesk.toml
cargo run -- gen-passwd admin
# Set [server].site_passwd, configure a target and its passwords (see below), then:
cargo run -- serve -c zdesk.toml
```

The copied example names `[passwords].example` but leaves it for you to add.
Set up the target's encrypted password and master-password file as described in
[Target passwords](docs/guide.md#target-passwords). For a target with an empty
password, add `example = ""` under `[passwords]` instead; it needs no master
password.

Cargo builds the frontend and compiles it into the binary, so the gateway is
the only thing that serves the page. The checks are:

```sh
cargo clippy --all-targets -- -D warnings
cargo test --lib
bun run --cwd frontend check
```

The self-contained integration tests under `tests/` run with `cargo test --tests`.
The container-backed VNC and wlshare tests are ignored by default because they
need Docker or Podman:

```sh
cargo test --test vnc_e2e --test wlshare_e2e -- --ignored
```

The RDP probes borrow a real Windows host, so they are examples run
on demand rather than tests (`cargo run --example rdp_client_probe -- <case>`),
and the headless browser tests are described in
[`tests/playwright`](tests/playwright/README.md).
[Development](docs/development.md) has the details, and
[Packaging](packaging/README.md) covers release builds.

## Licence

zDesk is under the MIT licence in [`LICENSE`](LICENSE).
