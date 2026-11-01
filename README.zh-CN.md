# zDesk

[English](README.md) | 简体中文 | [繁體中文](README.zh-TW.md)

在浏览器中使用远程桌面，连接 Windows 和 macOS 自带的远程桌面服务器，以及 Linux 上的 wlshare。zDesk 是一个小型网关，运行在你的机器旁边。它可以连接：

- **Windows 10 和 11 自带的远程桌面**，通过 RDP 及其现代图形管道。Windows 家庭版不能作为远程桌面主机。
- **macOS 内置的屏幕共享**，两种模式都支持：标准（Standard）和高性能（High Performance）。
- **Linux**，通过 [wlshare](https://github.com/andrewtheguy/wlshare/blob/main/README.zh-CN.md)，这是我们自己为 wlroots 合成器编写的 VNC 服务器。
- 其他 RDP 和 VNC 服务器，通过各协议的基础功能。

Windows 和 macOS 主机无需安装任何软件，只需打开系统自带的共享功能。网关提供一个显示桌面的网页，因此任何有浏览器的设备，无论是 Windows、macOS、Linux、iOS 还是 Android，同样无需安装客户端即可访问上述所有桌面。你的桌面因此不局限于某一台设备或某个地点：只要浏览器能访问网关，就能使用你的机器。

```text
浏览器 ── HTTPS 或 localhost ── zDesk ── RDP / VNC ── 你的桌面
```

`zdesk serve` 是单用户的：一个登录、一份目标列表、一个活动会话。原生安装包还包含 [`zdesk tui`](https://zdesk.dev/zh-CN/guides/zdesk#local-instances)，这是一个本地控制面，可以并排运行多个相互独立的网关实例。

> [!WARNING]
> **不保证向后兼容。** zDesk 的[方向](docs/architecture.md#direction)仍在调整中，任何版本都可能更改或移除某个配置项、WebSocket 协议或某项功能，且不再读取旧的写法。升级时请对照 [`zdesk.example.toml`](zdesk.example.toml) 检查你的配置。页面和网关属于同一个版本：升级时仍打开着的标签页会显示两者的版本号，并提示重新加载。这里说的是兼容性，而不是稳定性：我一直在实际使用未标注为实验性的功能，它们一直很稳定。

## 演示

Windows 上 Chrome 标签页中的 Mac 屏幕共享：

https://github.com/user-attachments/assets/fd0610bb-444a-4222-aa1d-76a39d09b3f0

在 [zDesk 网站](https://zdesk.dev/zh-CN/demo)观看更多演示。

## 功能

- **单一的浏览器客户端，无需安装任何东西。** 桌面上的 Chrome、Edge 或 Safari，Android 上的 Chrome，以及 iPhone 或 iPad 上的 Safari。在桌面上，可以从 Chrome 或 Edge 的菜单将页面安装为应用，使其拥有独立的窗口。
- **按浏览器调整尺寸，按其像素密度清晰显示。** Windows、高性能模式 Mac 以及 wlshare 的无头输出按屏幕像素密度跟随窗口，因此在 Retina 或 HiDPI 屏幕上清晰，并按真实尺寸显示。标准模式 Mac 的显示器保持其报告的密度。
- **同时显示两个显示器（alpha）**，支持 Windows、高性能模式 Mac 和 wlshare，第二个显示器在单独的浏览器标签页中打开。macOS 26 上 Apple 自己的查看器在有两个虚拟显示器时不提供动态分辨率。
- **远程自己的画面，在浏览器中解码。** Windows 的图形管道、高性能模式 Mac 的 HEVC 或 H.264、标准模式 Mac 的 Adaptive 画面以及 wlshare 的 H.264 都原样直通，除高性能模式 Mac 外，都按浏览器链路调节节奏。
- **最终收敛到无损的图块。** 由网关自己编码画面时，网关发送 64×64 图块：无损的部分用 PNG，运动的部分用 JPEG，静止后再无损地重发一次。
- **浏览器中的远程声音**：按浏览器链路调节的 Opus、适合局域网的无损 FLAC，或高性能模式 Mac 自己的 AAC-ELD。
- **双向剪贴板文本**，浏览器允许时自动同步，不允许时通过 Clipboard 面板。
- **摄像头和麦克风重定向（实验性）**，用于在选择器中勾选它们后启动的 RDP 或 wlshare 会话。
- **重新连接而不丢失桌面。** 重新加载或套接字断开后，拥有会话的浏览器有 60 秒可以重新连接。接管或之后的新连接会启动一个新会话，但 Windows、Mac 和 wlshare 会保持桌面运行，窗口仍在原处。
- **鼠标、键盘和触屏。** 安装为应用后，页面可以收到普通标签页留给浏览器的组合键。在手机或平板上，手指相当于触控板，支持双指缩放和软键盘。

## 支持的服务器

zDesk 围绕各平台原生的远程桌面服务器构建：Windows 自带的远程桌面、macOS 内置的屏幕共享，以及 Linux 上我们自己的 [wlshare](https://github.com/andrewtheguy/wlshare/blob/main/README.zh-CN.md)。Windows 和 macOS 优先：大多数人连接的是它们，所以功能首先在它们上面测试和优化。wlshare 紧随其后，不过它是三者中集成最无缝的，因为 RFB 缺少的功能都加到了它里面。

- **Windows 10 和 11 的远程桌面**（`rdp`）。按密度跟随窗口，传输 Opus 或 FLAC 声音，同时支持摄像头和麦克风。画面是其图形管道，直通给浏览器，可以是高质量，或（alpha）自适应 H.264。
- **Mac 的高性能屏幕共享**（`vnc`，`subtype = "ard-high-performance"`）。按密度跟随窗口，支持一个或两个虚拟显示器，始终传输 AAC-ELD 声音。画面是直通的 HEVC 或 H.264（beta），由 Mac 按自己的速率而非浏览器链路发送：静止画面数据很少，运动时很多。H.264 需要浏览器具备 `VideoDecoder`，但页面无需额外组件；HEVC 则需要能解码它的浏览器或页面的软件解码器。
- **Mac 的标准屏幕共享**（`vnc`，`subtype = "ard"`）。Mac 的物理显示器，单个或全部，按其自身尺寸和密度显示，画面是网关对 Mac 无损 Full 质量编码的图块，或直通的 Mac 自己的 Adaptive 画面（beta）；不支持声音、摄像头和麦克风。
- **Linux 上的 wlshare**（`vnc`，`subtype = "wlshare"`）。由于服务器是我们自己的，RFB 缺少的功能都加到了它里面：它可以切换输出、按密度跟随窗口、传输 Opus 或 FLAC 声音以及浏览器的摄像头和麦克风，并自行对整个桌面编码 H.264 供网关直通，按浏览器链路调节节奏；或者在链路带宽充裕时，使用它自己的无损图块，并为每个运动区域提供一路视频流，使用硬件或 libx264 编码的 H.264。可以同时显示两个输出，第二个在单独的浏览器标签页中（alpha）。

其他所有 VNC 服务器都是普通的 `vnc` 目标，通过 RFB 基础功能访问：一个 1x 帧缓冲，不调整尺寸，没有声音，由网关发送图块。其他 RDP 服务器可能碰巧能用，但未经测试。对比表、它们的共同点以及各自直通的内容，见 [Prioritized servers](docs/architecture.md#prioritized-servers)（英文）。

## 服务器和浏览器支持

各服务器和浏览器支持与不支持的功能，集中列在这里。

| | Windows 远程桌面 | Mac 高性能模式 | Mac 标准模式 | wlshare | 其他 VNC |
|---|---|---|---|---|---|
| 跟随浏览器窗口调整尺寸 | ✅ 需启用图形管道（默认） | ✅ | ❌ 物理显示器保持其自身尺寸 | ✅ 仅无头输出 | ❌ 服务器自身尺寸 |
| 按 HiDPI/Retina 密度清晰显示 | ✅ | ✅ | ✅ 按 Mac 报告的密度 | ✅ | ❌ 仅 1x |
| 第二个显示器在单独标签页中 | ⚠️ alpha，需启用图形管道 | ⚠️ alpha | ❌ 一次一个屏幕，从显示菜单切换，或所有屏幕合并在一个标签页中（alpha） | ⚠️ alpha，需两个输出 | ❌ |
| 声音 | ✅ Opus 或 FLAC | ✅ AAC-ELD，始终传输 | ❌ | ✅ Opus 或 FLAC | ❌ |
| 摄像头和麦克风 | ⚠️ 实验性 | ❌ | ❌ | ⚠️ 实验性 | ❌ |
| Unicode 剪贴板 | ✅ | ✅ | ✅ | ✅ | ⚠️ 需 Extended Clipboard，否则 Latin-1 |
| 画面按浏览器链路调节节奏 | ✅ | ❌ 按 Mac 自己的速率 | ✅ | ✅ | ✅ |
| 画面最终收敛到无损 | ⚠️ 仅当主机策略要求时 | ❌ | ✅ 使用 Full 质量时 | ⚠️ 使用 *Tiles with motion* 或 *Lossless tiles* 时 | ✅ |

| | Chrome、Edge | Safari | Firefox |
|---|---|---|---|
| 高性能模式 Mac 的 HEVC | ✅ 原生；在 Windows 上仅当 GPU 支持 HEVC 4:4:4 解码时，否则 hevc-wasm | ✅ 原生 | ⚠️ 仅 hevc-wasm |
| 高性能模式 Mac 的声音（AAC-ELD） | ✅ | ✅ 16.4 到 18 除外，它们完全没有声音 | ❌ |
| 无损 FLAC 声音 | ✅ | ❌ 仅 Opus | ✅ |
| 浏览器自己保留的组合键（⌘W、⌘T、⌘N） | ✅ 在应用窗口或沉浸式全屏中 | ❌ | ❌ |

hevc-wasm 是页面的软件 HEVC 解码器：网关在其安装目录的 `share/zdesk` 中找到发布归档时提供它，归档放在别处时由 `[hevc_wasm]` 指明。它和浏览器都无法解码 HEVC 时，如果浏览器具备 `VideoDecoder`，选择器提供 Mac 的 H.264（beta）；没有它时，两种画面都不提供，该目标无法启动。

应用窗口是从浏览器菜单安装的页面：在 Chrome 中为 ⋮ → *投放、保存和分享* → *将网页作为应用安装*；在 Edge 中为 ⋯ → *应用* → *将此站点作为应用安装*。它在独立的窗口中打开，没有标签页和地址栏，可以收到 ⌘W、⌘T、⌘N 等组合键。在标签页中使用 *Menu → Immersive full screen* 效果相同，还能收到 Alt+Tab 和 Super 键。

- **需要安全上下文（secure context）：** HTTPS，或直接或通过 SSH 隧道访问 `localhost`。通过普通 HTTP 访问局域网地址会被拒绝。
- **视频流和声音需要 WebCodecs。** 没有它时，只有网关的图块、标准模式 Mac 的 Adaptive 画面以及不含 H.264 的 RDP 管道可以使用，且没有声音。
- **高性能模式 Mac 的画面不随浏览器链路自适应：** 静止画面数据量很小，运动画面则很大。
- **每个网关只有一个活动会话。** 接管会结束它，并从选择器开始。
- **触屏相当于触控板：** 手指操作不会作为触摸屏触点发送到远程。
- **其他 RDP 服务器**可能碰巧能用，但未经测试。

## 快速开始

各版本之间不保证向后兼容（见上方的警告），因此请安装一个指定版本，并一直使用它，直到你决定升级。从[发布列表](https://github.com/andrewtheguy/zdesk/releases)中选一个版本，用它的版本号替换 `0.0.x`。在 Debian 13 或 Ubuntu 24.04 及更高版本上：

```sh
curl -fsSLO https://github.com/andrewtheguy/zdesk/releases/download/v0.0.x/zdesk-linux-amd64.deb
sudo apt install ./zdesk-linux-amd64.deb
```

安装后有两种运行方式：作为独立网关（`zdesk serve`）运行，读取你自己编写的配置；或使用控制面（`zdesk tui`），这是一个终端界面，在你使用的机器上运行网关。下面两节分别介绍。

### 独立网关：`zdesk serve`

用附带的示例创建配置：

```sh
sudo install -d -m 700 -o "$(id -un)" -g "$(id -gn)" /etc/zdesk /var/lib/zdesk
sudo install -m 600 -o "$(id -un)" -g "$(id -gn)" \
  /usr/share/doc/zdesk/zdesk.example.toml /etc/zdesk/zdesk.toml
```

把网页登录的用户名和一个目标写入 `/etc/zdesk/zdesk.toml`，并写上网关读取主密码的文件：

```toml
[server]
site_username = "admin"
site_passwd_hash = "$ZD;1;HMAC-SHA256;…"
master_password_file = "master-password"

[[targets]]
name = "workstation"
protocol = "rdp"
host = "192.0.2.10"
username = "Administrator"
password_ref = "workstation"
```

然后粘贴各个密码，它们都在你的主密码之下保存（见 [目标密码](https://zdesk.dev/zh-CN/guides/zdesk#target-passwords)）：`password master create` 询问新的主密码，把它写入该文件（只有你能读取），并输出配置所需的 `[master]` 块；`password hash` 输出网页登录密码的单向哈希，以 `site_passwd_hash = "…"` 的形式粘贴；`password encrypt` 输出目标的加密密码，以 `workstation = "…"` 的形式添加到 `[passwords]` 下。

```sh
zdesk password master create
zdesk password hash      # 网页登录的密码
zdesk password encrypt   # 目标的密码
```

启动它，然后打开 <http://localhost:52380>：

```sh
zdesk serve
```

页面需要安全上下文，因此从其他机器访问时，要把网关放在终止 TLS 的反向代理后面；`localhost` 可以直接使用。

另有 `.rpm`、macOS `.pkg`、Windows `.msi` 软件包以及容器镜像。镜像的配置设置了 `[server].master_password_file = "/run/secrets/zdesk_master"`，主密码挂载在该路径；[容器](docs/install.zh-CN.md#容器)介绍了如何创建该文件，以及如何用镜像输出配置的 `[master]` 和 `[passwords]`：

```sh
docker run -d --name zdesk -p 52380:52380 \
  -e ZDESK_LISTEN=0.0.0.0:52380 \
  -v ./zdesk.toml:/opt/zdesk/etc/zdesk.toml:ro \
  -v ./secrets:/run/secrets:ro \
  ghcr.io/andrewtheguy/zdesk:v0.0.x
```

### 控制面：`zdesk tui`

在浏览器和密钥环都在本机的桌面上（macOS、Windows，或带有 GNOME Keyring 或 KWallet 的 Linux），原生软件包的控制面可以改为由它来运行网关，每个网关都有自己的配置，无需上面的任何步骤：

```sh
zdesk tui
```

它只监听回环地址，位于 <http://zdesk.localhost:52380>。按 `n` 创建实例，按 `e` 在其配置中添加目标，按 `p` 加密目标的密码，按 `s` 启动实例，按 `o` 在浏览器中打开它。[本地实例](https://zdesk.dev/zh-CN/guides/zdesk#local-instances)列出了所有按键。容器镜像不包含控制面。

[安装 zDesk](docs/install.zh-CN.md) 涵盖所有平台、升级和卸载。

## 文档

- [使用 zDesk](https://zdesk.dev/zh-CN/guides/zdesk)：浏览器客户端、启动会话、设置 Windows、Mac、wlshare 和 VNC 目标、`zdesk tui`
- [设置 wlshare](https://zdesk.dev/zh-CN/guides/wlshare)：zDesk 连接的 Linux 服务器，运行在 wlroots 桌面上：安装、谁可以连接、开机即启动的无头桌面
- [安装 zDesk](docs/install.zh-CN.md)：软件包、首次配置、升级、卸载

以下文档只有英文版：

- [`zdesk.example.toml`](zdesk.example.toml)：所有配置项，带注释
- [Known issues](docs/known-issues.md)：值得认出、无需重新调查的已知问题
- [Roadmap](docs/roadmap.md)：计划做什么，以及不做什么
- [Architecture](docs/architecture.md)：设计及其约束、客户端协议、引擎
- [The RDP client](docs/rdp-client.md)、[Apple RFB 003.889](docs/apple-vnc-889.md)：各协议的实现和测量结果
- [Development](docs/development.md)：从源码运行、检查、端到端测试、构建
- [Packaging](packaging/README.md)：软件包布局、构建脚本、发布

## 开发

需要带 rustup 的 Rust 和 [Bun](https://bun.sh)。

```sh
bun install --cwd frontend
cp zdesk.example.toml zdesk.toml
# 设置 [server].site_username，配置目标和各个密码（见下文），然后：
cargo run -- serve -c zdesk.toml
```

复制的示例引用了 `[passwords].example`，但该值和 `[server].site_passwd_hash` 需要你自己添加。按[目标密码](https://zdesk.dev/zh-CN/guides/zdesk#target-passwords)设置主密码文件和各个密码：`password encrypt` 为每个目标输出对应的值，`password hash` 输出网页登录密码的值。如果目标使用空密码，可在 `[passwords]` 下添加 `example = ""`。

Cargo 会构建前端并将其编译进二进制文件，因此网关是唯一提供页面的程序。检查命令：

```sh
cargo clippy --all-targets -- -D warnings
cargo test --lib
bun run --cwd frontend check
```

`tests/` 下自包含的集成测试用 `cargo test --tests` 运行。依赖容器的 VNC 和 wlshare 测试默认被忽略，需要 Docker 或 Podman（`cargo test --test vnc_e2e --test wlshare_e2e -- --ignored`）。RDP 探针借用一台真实的 Windows 主机，因此它们是按需运行的示例而不是测试（`cargo run --example rdp_client_probe -- <case>`），无头浏览器测试见 [`tests/playwright`](tests/playwright/README.md)。详情见 [Development](docs/development.md)，发布构建见 [Packaging](packaging/README.md)。

## 许可证

zDesk 采用 [`LICENSE`](LICENSE) 中的 MIT 许可证。
