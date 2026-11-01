# zDesk

[English](README.md) | [简体中文](README.zh-CN.md) | 繁體中文

在瀏覽器中使用遠端桌面，連線 Windows 和 macOS 內建的遠端桌面伺服器，以及 Linux 上的 wlshare。zDesk 是一個小型閘道，執行在你的電腦旁邊。它可以連線到：

- **Windows 10 和 11 內建的遠端桌面**，透過 RDP 及其新式圖形管線。Windows 家用版無法作為遠端桌面主機。
- **macOS 內建的螢幕共享**，兩種模式都支援：標準（Standard）和高效能（High Performance）。
- **Linux**，透過 [wlshare](https://github.com/andrewtheguy/wlshare/blob/main/README.zh-TW.md)，這是我們自己為 wlroots 合成器撰寫的 VNC 伺服器。
- 其他 RDP 和 VNC 伺服器，透過各協定的基本功能。

Windows 和 macOS 主機不需要安裝任何軟體，只要開啟系統內建的共享功能即可。閘道提供一個顯示桌面的網頁，因此任何有瀏覽器的裝置，無論是 Windows、macOS、Linux、iOS 還是 Android，同樣不需要安裝用戶端就能存取上述所有桌面。你的桌面因此不侷限於某一台裝置或某個地點：只要瀏覽器能連到閘道，就能使用你的電腦。

```text
瀏覽器 ── HTTPS 或 localhost ── zDesk ── RDP / VNC ── 你的桌面
```

`zdesk serve` 是單一使用者的：一組登入、一份目標清單、一個進行中的工作階段。原生安裝套件還包含 [`zdesk tui`](https://zdesk.dev/zh-TW/guide#local-instances)，這是一個本機控制平面，可以並排執行多個彼此獨立的閘道執行個體。

> [!WARNING]
> **不保證回溯相容。** zDesk 的[方向](docs/architecture.md#direction)仍在調整中，任何版本都可能變更或移除某個設定項、WebSocket 協定或某項功能，且不再讀取舊的寫法。升級時請對照 [`zdesk.example.toml`](zdesk.example.toml) 檢查你的設定。頁面和閘道屬於同一個版本：升級時仍開著的分頁會顯示兩者的版本號，並提示重新載入。這裡說的是相容性，而不是穩定性：我一直在實際使用未標示為實驗性的功能，它們一直很穩定。

## 示範

在 Windows 的 Chrome 分頁中顯示 Mac 的螢幕共享：

https://github.com/user-attachments/assets/fd0610bb-444a-4222-aa1d-76a39d09b3f0

在 [zDesk 網站](https://zdesk.dev/zh-TW/demo)觀看更多示範。

## 功能

- **單一的瀏覽器用戶端，不需要安裝任何東西。** 桌面上的 Chrome、Edge 或 Safari，Android 上的 Chrome，以及 iPhone 或 iPad 上的 Safari。在桌面上，可以從 Chrome 或 Edge 的選單將頁面安裝為應用程式，讓它擁有獨立的視窗。
- **依瀏覽器調整尺寸，依其像素密度清晰顯示。** Windows、高效能模式 Mac 以及 wlshare 的無頭輸出會依螢幕像素密度跟隨視窗，因此在 Retina 或 HiDPI 螢幕上清晰，並以實際尺寸顯示。標準模式 Mac 的顯示器維持其回報的密度。
- **同時顯示兩個顯示器（alpha）**，支援 Windows、高效能模式 Mac 和 wlshare，第二個顯示器在另一個瀏覽器分頁中開啟。macOS 26 上 Apple 自己的檢視器在有兩個虛擬顯示器時不提供動態解析度。
- **遠端自己的畫面，在瀏覽器中解碼。** Windows 的圖形管線、高效能模式 Mac 的 HEVC 或 H.264、標準模式 Mac 的 Adaptive 畫面以及 wlshare 的 H.264 都原封不動地直通，除了高效能模式 Mac 之外，都依瀏覽器連線調節節奏。
- **最終收斂為無損的圖塊。** 由閘道自行編碼畫面時，閘道傳送 64×64 圖塊：無損的部分用 PNG，移動中的部分用 JPEG，靜止後再無損地重送一次。
- **瀏覽器中的遠端聲音**：依瀏覽器連線調節的 Opus、適合區域網路的無損 FLAC，或高效能模式 Mac 自己的 AAC-ELD。
- **雙向剪貼簿文字**，瀏覽器允許時自動同步，不允許時透過 Clipboard 面板。
- **攝影機和麥克風重新導向（實驗性）**，用於在選擇器中勾選它們後啟動的 RDP 或 wlshare 工作階段。
- **重新連線而不遺失桌面。** 重新載入或通訊端中斷後，擁有工作階段的瀏覽器有 60 秒可以重新連線。接管或之後的新連線會啟動一個新的工作階段，但 Windows、Mac 和 wlshare 會讓桌面繼續執行，視窗仍在原處。
- **滑鼠、鍵盤和觸控。** 安裝為應用程式後，頁面可以收到一般分頁保留給瀏覽器的快速鍵。在手機或平板上，手指相當於觸控板，支援雙指縮放和螢幕鍵盤。

## 支援的伺服器

zDesk 以各平台原生的遠端桌面伺服器為核心打造：Windows 內建的遠端桌面、macOS 內建的螢幕共享，以及 Linux 上我們自己的 [wlshare](https://github.com/andrewtheguy/wlshare/blob/main/README.zh-TW.md)。Windows 和 macOS 優先：大多數人連線的是它們，所以功能先在它們上面測試和最佳化。wlshare 緊隨其後，不過它是三者中整合最無縫的，因為 RFB 缺少的功能都加到了它裡面。

- **Windows 10 和 11 的遠端桌面**（`rdp`）。依密度跟隨視窗，傳輸 Opus 或 FLAC 聲音，同時支援攝影機和麥克風。畫面是其圖形管線，直通給瀏覽器，可以是高品質，或（alpha）自適應 H.264。
- **Mac 的高效能螢幕共享**（`vnc`，`subtype = "ard-high-performance"`）。依密度跟隨視窗，支援一個或兩個虛擬顯示器，一律傳輸 AAC-ELD 聲音。畫面是直通的 HEVC 或 H.264（beta），由 Mac 依自己的速率而非瀏覽器連線傳送：靜止畫面資料很少，移動時很多。H.264 需要瀏覽器具備 `VideoDecoder`，但頁面不需要額外元件；HEVC 則需要能解碼它的瀏覽器或頁面的軟體解碼器。
- **Mac 的標準螢幕共享**（`vnc`，`subtype = "ard"`）。Mac 的實體顯示器，單一或全部，依其自身尺寸和密度顯示，畫面是閘道針對 Mac 無損 Full 品質編碼的圖塊，或直通的 Mac 自己的 Adaptive 畫面（beta）；不支援聲音、攝影機和麥克風。
- **Linux 上的 wlshare**（`vnc`，`subtype = "wlshare"`）。由於伺服器是我們自己的，RFB 缺少的功能都加到了它裡面：它可以切換輸出、依密度跟隨視窗、傳輸 Opus 或 FLAC 聲音以及瀏覽器的攝影機和麥克風，並自行針對整個桌面編碼 H.264 供閘道直通，依瀏覽器連線調節節奏；或者在連線頻寬充裕時，使用它自己的無損圖塊，並為每個移動區域提供一路視訊串流，使用硬體或 libx264 編碼的 H.264。可以同時顯示兩個輸出，第二個在另一個瀏覽器分頁中（alpha）。

其他所有 VNC 伺服器都是一般的 `vnc` 目標，透過 RFB 基本功能存取：一個 1x 影格緩衝區，不調整尺寸，沒有聲音，由閘道傳送圖塊。其他 RDP 伺服器或許剛好能用，但未經測試。比較表、它們的共同點以及各自直通的內容，請見 [Prioritized servers](docs/architecture.md#prioritized-servers)（英文）。

## 伺服器與瀏覽器支援

各伺服器和瀏覽器支援與不支援的功能，集中列在這裡。

| | Windows 遠端桌面 | Mac 高效能模式 | Mac 標準模式 | wlshare | 其他 VNC |
|---|---|---|---|---|---|
| 跟隨瀏覽器視窗調整尺寸 | ✅ 需啟用圖形管線（預設） | ✅ | ❌ 實體顯示器維持其自身尺寸 | ✅ 僅無頭輸出 | ❌ 伺服器自身尺寸 |
| 以 HiDPI/Retina 密度清晰顯示 | ✅ | ✅ | ✅ 依 Mac 回報的密度 | ✅ | ❌ 僅 1x |
| 第二個顯示器在另一個分頁中 | ⚠️ alpha，需啟用圖形管線 | ⚠️ alpha | ❌ 一次一個螢幕，從顯示選單切換，或所有螢幕合併在一個分頁中（alpha） | ⚠️ alpha，需兩個輸出 | ❌ |
| 聲音 | ✅ Opus 或 FLAC | ✅ AAC-ELD，一律傳輸 | ❌ | ✅ Opus 或 FLAC | ❌ |
| 攝影機和麥克風 | ⚠️ 實驗性 | ❌ | ❌ | ⚠️ 實驗性 | ❌ |
| Unicode 剪貼簿 | ✅ | ✅ | ✅ | ✅ | ⚠️ 需 Extended Clipboard，否則為 Latin-1 |
| 畫面依瀏覽器連線調節節奏 | ✅ | ❌ 依 Mac 自己的速率 | ✅ | ✅ | ✅ |
| 畫面最終收斂為無損 | ⚠️ 僅當主機原則要求時 | ❌ | ✅ 使用 Full 品質時 | ⚠️ 使用 *Tiles with motion* 或 *Lossless tiles* 時 | ✅ |

| | Chrome、Edge | Safari | Firefox |
|---|---|---|---|
| 高效能模式 Mac 的 HEVC | ✅ 原生；在 Windows 上僅當 GPU 支援 HEVC 4:4:4 解碼時，否則為 hevc-wasm | ✅ 原生 | ⚠️ 僅 hevc-wasm |
| 高效能模式 Mac 的聲音（AAC-ELD） | ✅ | ✅ 16.4 到 18 除外，它們完全沒有聲音 | ❌ |
| 無損 FLAC 聲音 | ✅ | ❌ 僅 Opus | ✅ |
| 瀏覽器自己保留的快速鍵（⌘W、⌘T、⌘N） | ✅ 在應用程式視窗或沉浸式全螢幕中 | ❌ | ❌ |

hevc-wasm 是頁面的軟體 HEVC 解碼器：閘道在其安裝目錄的 `share/zdesk` 中找到發行封存檔時提供它，封存檔放在別處時由 `[hevc_wasm]` 指定。它和瀏覽器都無法解碼 HEVC 時，如果瀏覽器具備 `VideoDecoder`，選擇器會提供 Mac 的 H.264（beta）；沒有它時，兩種畫面都不提供，該目標無法啟動。

應用程式視窗是從瀏覽器選單安裝的頁面：在 Chrome 中為 ⋮ → *投放、儲存及分享* → *將網頁安裝為應用程式*；在 Edge 中為 ⋯ → *應用程式* → *將此網站安裝為應用程式*。它在獨立的視窗中開啟，沒有分頁和網址列，可以收到 ⌘W、⌘T、⌘N 等快速鍵。在分頁中使用 *Menu → Immersive full screen* 效果相同，還能收到 Alt+Tab 和 Super 鍵。

- **需要安全環境（secure context）：** HTTPS，或直接或透過 SSH 通道存取 `localhost`。透過一般 HTTP 存取區域網路位址會被拒絕。
- **視訊串流和聲音需要 WebCodecs。** 沒有它時，只有閘道的圖塊、標準模式 Mac 的 Adaptive 畫面以及不含 H.264 的 RDP 管線可以使用，而且沒有聲音。
- **高效能模式 Mac 的畫面不隨瀏覽器連線自適應：** 靜止畫面資料量很小，移動畫面則很大。
- **每個閘道只有一個進行中的工作階段。** 接管會結束它，並從選擇器開始。
- **觸控相當於觸控板：** 手指操作不會以觸控螢幕觸點的形式傳送到遠端。
- **其他 RDP 伺服器**或許剛好能用，但未經測試。

## 快速入門

各版本之間不保證回溯相容（見上方的警告），因此請安裝一個指定的版本，並持續使用它，直到你決定升級為止。從[發行版清單](https://github.com/andrewtheguy/zdesk/releases)中挑選一個版本，以它的版本號碼取代 `0.0.x`。在 Debian 13 或 Ubuntu 24.04 及更新版本上：

```sh
curl -fsSLO https://github.com/andrewtheguy/zdesk/releases/download/v0.0.x/zdesk-linux-amd64.deb
sudo apt install ./zdesk-linux-amd64.deb
```

安裝後有兩種執行方式：作為獨立閘道（`zdesk serve`）執行，讀取你自己撰寫的設定；或使用控制平面（`zdesk tui`），這是一個終端機介面，在你使用的電腦上執行閘道。下面兩節分別說明。

### 獨立閘道：`zdesk serve`

用隨附的範例建立設定檔：

```sh
sudo install -d -m 700 -o "$(id -un)" -g "$(id -gn)" /etc/zdesk /var/lib/zdesk
sudo install -m 600 -o "$(id -un)" -g "$(id -gn)" \
  /usr/share/doc/zdesk/zdesk.example.toml /etc/zdesk/zdesk.toml
```

把網頁登入的使用者名稱和一個目標寫入 `/etc/zdesk/zdesk.toml`，並寫上閘道讀取主密碼的檔案：

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

接著貼上各個密碼，它們都在你的主密碼之下保存（請見[目標密碼](https://zdesk.dev/zh-TW/guide#target-passwords)）：`password master create` 詢問新的主密碼，把它寫入該檔案（只有你能讀取），並輸出設定所需的 `[master]` 區塊；`password hash` 輸出網頁登入密碼的單向雜湊，以 `site_passwd_hash = "…"` 的形式貼上；`password encrypt` 輸出目標的加密密碼，以 `workstation = "…"` 的形式加到 `[passwords]` 下。

```sh
zdesk password master create
zdesk password hash      # 網頁登入的密碼
zdesk password encrypt   # 目標的密碼
```

啟動它，然後開啟 <http://localhost:52380>：

```sh
zdesk serve
```

頁面需要安全環境，因此從其他電腦存取時，要把閘道放在終止 TLS 的反向代理伺服器後面；`localhost` 可以直接使用。

另有 `.rpm`、macOS `.pkg`、Windows `.msi` 套件以及容器映像檔。映像檔的設定指定了 `[server].master_password_file = "/run/secrets/zdesk_master"`，主密碼掛載在該路徑；[容器](docs/install.zh-TW.md#容器)說明了如何建立該檔案，以及如何用映像檔輸出設定的 `[master]` 和 `[passwords]`：

```sh
docker run -d --name zdesk -p 52380:52380 \
  -e ZDESK_LISTEN=0.0.0.0:52380 \
  -v ./zdesk.toml:/opt/zdesk/etc/zdesk.toml:ro \
  -v ./secrets:/run/secrets:ro \
  ghcr.io/andrewtheguy/zdesk:v0.0.x
```

### 控制平面：`zdesk tui`

在瀏覽器和金鑰圈都在本機的桌面上（macOS、Windows，或具備 GNOME Keyring 或 KWallet 的 Linux），原生安裝套件的控制平面可以改由它來執行閘道，每個閘道都有自己的設定，不需要上面的任何步驟：

```sh
zdesk tui
```

它只監聽回送位址，位於 <http://zdesk.localhost:52380>。按 `n` 建立執行個體，按 `e` 在其設定中新增目標，按 `p` 加密目標的密碼，按 `s` 啟動執行個體，按 `o` 在瀏覽器中開啟它。[本機執行個體](https://zdesk.dev/zh-TW/guide#local-instances)列出了所有按鍵。容器映像檔不包含控制平面。

[安裝 zDesk](docs/install.zh-TW.md) 涵蓋所有平台、升級和解除安裝。

## 文件

- [使用 zDesk](https://zdesk.dev/zh-TW/guide)：瀏覽器用戶端、啟動工作階段、設定 Windows、Mac、wlshare 和 VNC 目標、`zdesk tui`
- [安裝 zDesk](docs/install.zh-TW.md)：套件、首次設定、升級、解除安裝

以下文件只有英文版：

- [`zdesk.example.toml`](zdesk.example.toml)：所有設定項，附註解
- [Known issues](docs/known-issues.md)：值得認得、不必重新調查的已知問題
- [Roadmap](docs/roadmap.md)：計畫做什麼，以及不做什麼
- [Architecture](docs/architecture.md)：設計及其限制、用戶端協定、引擎
- [The RDP client](docs/rdp-client.md)、[Apple RFB 003.889](docs/apple-vnc-889.md)：各協定的實作和測量結果
- [Development](docs/development.md)：從原始碼執行、檢查、端對端測試、建置
- [Packaging](packaging/README.md)：套件配置、建置指令碼、發行

## 開發

需要附 rustup 的 Rust 和 [Bun](https://bun.sh)。

```sh
bun install --cwd frontend
cp zdesk.example.toml zdesk.toml
# 設定 [server].site_username，設定目標和各個密碼（請見下文），然後：
cargo run -- serve -c zdesk.toml
```

複製的範例參照了 `[passwords].example`，但該值和 `[server].site_passwd_hash` 需要你自己加上。請依[目標密碼](https://zdesk.dev/zh-TW/guide#target-passwords)設定主密碼檔案和各個密碼：`password encrypt` 會為每個目標輸出對應的值，`password hash` 則輸出網頁登入密碼的值。如果目標使用空白密碼，可在 `[passwords]` 下加上 `example = ""`。

Cargo 會建置前端並將其編譯進二進位檔，因此閘道是唯一提供頁面的程式。檢查指令：

```sh
cargo clippy --all-targets -- -D warnings
cargo test --lib
bun run --cwd frontend check
```

`tests/` 下自成一體的整合測試用 `cargo test --tests` 執行。依賴容器的 VNC 和 wlshare 測試預設會被略過，需要 Docker 或 Podman（`cargo test --test vnc_e2e --test wlshare_e2e -- --ignored`）。RDP 探測程式會借用一台真實的 Windows 主機，因此它們是依需求執行的範例而不是測試（`cargo run --example rdp_client_probe -- <case>`），無頭瀏覽器測試請見 [`tests/playwright`](tests/playwright/README.md)。詳情請見 [Development](docs/development.md)，發行版建置請見 [Packaging](packaging/README.md)。

## 授權

zDesk 採用 [`LICENSE`](LICENSE) 中的 MIT 授權條款。
