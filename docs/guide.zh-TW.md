# 使用 zDesk

[English](guide.md) | [简体中文](guide.zh-CN.md) | 繁體中文

閘道[安裝](install.zh-TW.md)好之後，維運人員和使用者需要了解的內容：如何存取頁面、工作階段以哪些選項啟動、各類目標如何設定，以及本機多執行個體控制平面。所有設定項都在附註解的 [`zdesk.example.toml`](../zdesk.example.toml) 中，背後的設計請見 [Architecture](architecture.md)。本頁連結的其他文件除安裝文件外都只有英文版。

## 用戶端

用戶端就是閘道提供的頁面，編譯在閘道的二進位檔中。它需要：

- **安全環境（secure context）。** 閘道只使用純 HTTP，沒有 TLS 監聽器，因此需要透過回送位址（`localhost`、`127.0.0.1`、`[::1]`、任何 `.localhost` 名稱）或終止 TLS 的反向代理伺服器存取。透過純 `http://` 存取區域網路位址會被拒絕，頁面會說明原因。
- **WebCodecs**，用於遠端以視訊形式傳送的畫面：`VideoDecoder` 用於 wlshare 的 VP9、高效能模式 Mac 的 HEVC 或 H.264，以及 RDP 主機的 H.264。沒有它時，用戶端本身仍然可以啟動，閘道的圖塊、標準模式 Mac 的 Adaptive 畫面以及不含 H.264 的 RDP 管線在任何瀏覽器中都能運作。頁面會事先詢問瀏覽器的解碼器能否解碼 Mac 的 HEVC、RDP 主機的 H.264 和 wlshare 的 VP9：解碼器拒絕前兩者時，選擇器將其變灰並說明原因；解碼器拒絕 VP9 時，選擇器將 `wlshare` 目標的 Video 變灰，仍可使用它的帶移動視訊的圖塊，其移動區域是 H.264，具備 `VideoDecoder` 的瀏覽器即被視為能夠解碼，以及它的漸進式圖塊，由頁面自行解碼。Mac 的 H.264 不事先詢問：它提供給所有具備 `VideoDecoder` 的瀏覽器，沒有時變灰，解碼器不接受它時會在工作階段中透過解碼器的錯誤說明。`AudioDecoder` 播放 Opus、Mac 的 AAC-ELD 和 FLAC；沒有它的瀏覽器（Safari 16.4 到 18）不播放聲音，較新的 Safari 的 `AudioDecoder` 無法解碼 FLAC，但可以播放 Opus 和 Mac 的聲音。

在桌面上使用時，請在 Chrome 或 Edge 中把頁面安裝為應用程式。應用程式視窗會把一般分頁自己保留的快速鍵（⌘W、⌘T、⌘N 等）交給頁面。在分頁中，**Menu → Immersive full screen** 也能做到這一點，而且還能收到視窗通常收不到的按鍵，例如 Super 鍵和 Alt+Tab。

桌面以 100% 顯示，從不縮放以符合視窗：較大的桌面會捲動；而在以跟隨視窗方式啟動的工作階段中，則由視窗決定遠端的尺寸。手機或平板會符合寬度並支援雙指縮放，手指作為觸控板，並提供螢幕鍵盤。

已安裝的應用程式有 **Window → Size to _width_×_height_**，它透過 [`window.resizeTo()`](https://developer.mozilla.org/en-US/docs/Web/API/Window/resizeTo) 把本機視窗調整為遠端桌面的尺寸。它只調整本機視窗，從不縮放或調整遠端桌面。它在已安裝的 Chrome 和 Edge 應用程式視窗中有效，在其他瀏覽器中則盡力而為；一般分頁無法調整視窗尺寸，行動瀏覽器會忽略這個要求。

在 macOS 上，視窗的圓角會遮住頁面，因此與桌面同尺寸的視窗會遺失影格緩衝區四角的像素。圓角半徑是系統層級的設定；設為 1 點就夠方正，每個像素都看得到：

```sh
defaults write -g NSConvolutionOverride1 -float 1
```

結束並重新開啟瀏覽器應用程式後生效。`defaults delete -g NSConvolutionOverride1` 會恢復 Apple 的半徑：macOS 26 上為 26 點，之前為 10 點。

## 啟動工作階段

登入後頁面會顯示目標選擇器（picker）。點選一個目標會將它展開，下方顯示選項，按下 Start 即以這些選項連線。這些選項依工作階段選擇，在工作階段期間維持不變，並由瀏覽器依目標記住；它們都不是設定項。

| 目標 | 視窗決定尺寸 | 聲音 | 直通 | 圖形 |
|---|---|---|---|---|
| `rdp` | 是 | 關、Opus 或 FLAC | 圖形管線，一律直通 | 高品質或自適應 H.264 |
| `rdp` 且 `egfx = false` | 否 | 關、Opus 或 FLAC | — | — |
| `vnc` | 否 | 無 | — | — |
| `vnc`，`wlshare` | 是 | 關、Opus 或 FLAC | 其 VP9、其帶移動視訊的圖塊，或其漸進式圖塊（beta），一律直通 | — |
| `vnc`，`ard` | 否 | 無 | Mac 的 Adaptive 畫面直通（beta），或不直通：閘道針對 Mac Full 品質編碼的圖塊 | — |
| `vnc`，`ard` 且 `virtual_display` | 是 | 無 | 同 `ard` | — |
| `vnc`，`ard-high-performance` | 是 | 一律傳輸 | Mac 的 HEVC，或其 H.264（beta），一律直通 | — |

- **尺寸（Size）。** 桌面要不是維持一個固定尺寸，即目標的 `size = "1920x1080"`，未設定時為 1440×900；就是跟隨視窗：要求遠端依視窗的尺寸和瀏覽器的像素密度算繪，而不是在用戶端縮放。固定尺寸同樣跟隨瀏覽器的密度。手機只提供固定尺寸。一般 `vnc` 目標以及使用 Mac 實體顯示器的 `ard` 依遠端自身的尺寸顯示，不接受 `size`。
- **聲音（Sound）。** Opus，或無損的 FLAC。不帶聲音啟動的工作階段不向遠端要求聲音，因此主機會繼續在原本的地方播放。工作階段中的 Mute 和 Unmute 只改變這個瀏覽器是否收聽。手機、平板和 Safari 以靜音啟動。沒有 `AudioDecoder` 的瀏覽器會把 Sound 變灰；Safari 的 `AudioDecoder` 無法解碼 FLAC，會把 FLAC 變灰，因此在那裡勾選 Sound 就是使用 Opus。在高效能模式 Mac 上，這類瀏覽器中選單的 Audio 按鈕會顯示 *Audio not supported*，無法解碼 Mac 的 AAC-ELD 的瀏覽器中則顯示 *AAC-ELD not supported*。
- **直通（Passthrough）。** 把遠端自己的串流原封不動地傳給瀏覽器，而不是由閘道編碼成圖塊。高效能模式 Mac 的畫面一律直通，可以選擇的是哪一路串流：其 HEVC，瀏覽器無法接收時會變灰；或其 H.264（beta），它提供給所有具備 `VideoDecoder` 的瀏覽器，頁面中不需要 HEVC 解碼器。在啟用管線的 `rdp` 目標上，這不是一個選項：選擇器會說明它一律直通，沒有可勾選的方塊，無法合成管線的瀏覽器不能啟動該目標。直通的 RDP 管線會跟隨瀏覽器的連線：瀏覽器跟不上時，Windows 主機會繪製較少的影格，在 Adaptive 下還會較粗糙。Mac 直通的串流則不會：Mac 依自己的速率傳送，靜止畫面很少，移動時很多。在 `ard` 目標上，Picture 群組為 *Apple's Adaptive (beta), passed through*（預設）或 *Gateway adaptive, settles to lossless*。Apple's Adaptive 是 Mac 自己的 8×8 圖塊，由本頁面解碼：每個變化區域先來一遍粗略的，接著是它的細化；在任何連線上精細度都相同，較慢的連線只是更新較少，並在畫面靜止後才得到細化；它永遠達不到無損。以它啟動的工作階段在瀏覽器回來時會重新開始而不是恢復，與直通的 RDP 管線一樣。Gateway adaptive 是 Mac 的無損 Full 品質，由閘道解壓縮後依 `video_quality` 以圖塊傳送，與它傳送任何 VNC 伺服器的桌面一樣：區域變化時用 JPEG，穩定後變為精確，配合連線調整，並在瀏覽器回來時恢復。在 Combined Display（請見下文）下 Apple's Adaptive 會變灰，因為 Combined Display 同樣是閘道的圖塊。在 `wlshare` 目標上，Picture 群組為 *Video, passed through*（預設），即 wlshare 針對整個桌面編碼的 VP9；*Tiles with motion, passed through*，即 wlshare 自己的圖塊，適用於區域網路；或 *Progressive tiles (beta), passed through*，即依連線調整、像 Windows 主機在沒有 H.264 時那樣繪製的圖塊：請見 [wlshare](#wlshare-vnc-subtype--wlshare)。
- **顯示器（Displays）。** 在共享 Mac 實體螢幕的 `ard` 目標上：*One at a time*（beta，預設）從主螢幕開始，在顯示器選單中提供其他螢幕，畫面採用上面選擇的方式；*Combined*（alpha）同時顯示所有螢幕，相當於 Apple 檢視器的 Combined Display，不提供單一螢幕。Combined Display 向 Mac 要求 Full 品質，由閘道解碼、合成並依 `video_quality` 以圖塊傳送，與閘道編碼的任何畫面一樣跟隨瀏覽器的連線。在 1x 瀏覽器上，2x 螢幕由閘道縮小為 1x，因此只占用其自身像素四分之一的連線頻寬；2x 瀏覽器收到 Mac 原生的像素，1x 螢幕由瀏覽器自己放大繪製。僅螢幕共享的標準模式提供。請見 [A passed stream and the browser's link](architecture.md#a-passed-stream-and-the-browsers-link)。
- **圖形（Graphics）。** RDP 主機如何繪製其管線。*High quality*（預設）使用管線中除了 H.264 以外的編碼，適用於區域網路：慢速連線只會得到較少的影格，而不會較粗糙。*Adaptive*（alpha）是主機依瀏覽器連線調整的 H.264：慢速連線較粗糙，快速連線較精細。其下，*H.264 full screen*（預設）用 H.264 繪製整個桌面，*H.264 region* 只對像影片那樣移動的部分使用 H.264，其餘部分依 High quality 的方式繪製，文字因此保持清晰，但自適應較少。在 `VideoDecoder` 無法解碼主機 H.264 的瀏覽器中，Adaptive 會變灰。要讓主機無損繪製，請見[精確畫面](#精確畫面)。
- **攝影機（Camera）** 和 **麥克風（Microphone）**（實驗性）。在 `rdp` 或 `wlshare` 目標上，這兩個勾選項會向遠端提供這個瀏覽器的裝置：兩者都在整個工作階段中提供，遠端有程式使用時才開啟。請見[攝影機和麥克風](#攝影機和麥克風)。

一個閘道只持有一個工作階段。重新載入或連線中斷通常會恢復工作階段；啟用圖形管線的 RDP 工作階段以及標準模式 Mac 的 Adaptive 工作階段則會重新連線引擎，因為瀏覽器的合成器或解碼器狀態已經遺失。另一個瀏覽器若發現擁有者仍連線中，會被詢問是否接管；在擁有者重新連線的寬限期內到達的瀏覽器可以直接占用該位置。兩種情況都從選擇器開始，使用它自己的選項。遠端會保留自己的桌面，因此再次啟動該目標會回到相同的視窗。

請見 [What a session is started with](architecture.md#what-a-session-is-started-with)。

## 目標密碼

目標從不寫出自己的密碼。它只提供密碼的名稱，設定在 `[passwords]` 中以該名稱儲存加密後的密碼：

```toml
[[targets]]
name = "workstation"
protocol = "rdp"
host = "192.0.2.10"
username = "Administrator"
password_ref = "workstation"
```

這兩個表格由指令輸出供你貼上，從不手動輸入：

```sh
zdesk password master create -c zdesk.toml   # 一次：寫入主密碼檔案，輸出 [master]
zdesk password master show -c zdesk.toml     # 之後每個設定：輸出該主密碼下的 [master]
zdesk password encrypt -c zdesk.toml    # 輸出 [passwords] 的一個值
```

```toml
[master]
salt = "…"
check = "…"

[passwords]
workstation = "$ZD;1;AES-256-GCM;…"
```

`password encrypt` 會要求輸入兩次密碼，或從管道讀取一行，並用設定中已有的 `[master]` 加密，它只讀取設定。名稱在貼上時由你決定，也就是 `password_ref` 寫的名稱。`vnc_password_ref` 以同樣方式參照 VNC 伺服器自己的密碼。兩個輸出中都沒有機密資訊，因此可以放在任何組裝設定的地方：編輯器、Helm values 檔案、範本化的 ConfigMap。本機執行個體的設定由 TUI 負責寫入：它的 `p` 會詢問名稱和密碼，並自己寫入兩個表格。

值也可以留空，`workstation = ""`，作為尚未設定之密碼的預留位置：在貼上真實的值之前，目標以空白密碼回應；如果 `[passwords]` 中的值全是這樣的預留位置，設定就不需要 `[master]` 或主密碼。

其他所有值都用 AES-256-GCM 加密，金鑰由你的**主密碼**和設定自己的 salt 衍生。提供服務的閘道從 `[server].master_password_file` 指定的檔案讀取主密碼：可以是容器的 secret、systemd 的 credential，或只有閘道帳號能讀取的檔案。該檔案只包含主密碼；結尾的換行會被忽略。`password master create` 寫入該檔案，只有其使用者能讀取；檔案已存在時拒絕取代，除非指定 `--force`，取代後用舊主密碼加密的每個值都無法再開啟。`password master show` 輸出既有主密碼下的 `[master]` 區塊。TUI 把自己的主密碼儲存在鑰匙圈中（macOS 鑰匙圈、Windows 認證管理員，或 Linux 桌面的 Secret Service：GNOME Keyring、KWallet），在 TUI 啟動時讀取一次，沒有時在那時產生，因此你從不需要輸入。除了 TUI 以外，沒有任何東西會讀取鑰匙圈。

| 啟動方式 | 主密碼來源 |
|---|---|
| `zdesk serve` | `[server].master_password_file`；目標參照加密值而未設定它時拒絕啟動，從不詢問 |
| `zdesk tui` | 鑰匙圈，在 TUI 啟動時讀取；沒有時產生並儲存在那裡；鑰匙圈無回應時拒絕啟動，從不詢問 |
| `zdesk password` | `[server].master_password_file`，否則詢問；`password master create` 詢問兩次並寫入該檔案 |

本機執行個體沒有 `[server]` 區塊，因此其主密碼一律來自鑰匙圈。

主密碼錯誤會在啟動時回報。更換主密碼代表要在新的 `[master]` 下重新設定每個密碼。

## Windows (`rdp`)

```toml
[[targets]]
name = "workstation"
protocol = "rdp"
host = "192.0.2.10"
username = "Administrator"
password_ref = "workstation"
```

RDP 用戶端是閘道自己實作的，已針對 Windows 10 和 11 的遠端桌面（NLA）測試。它傳輸桌面、指標、鍵盤、滑鼠、調整尺寸、剪貼簿、聲音，以及瀏覽器的攝影機和麥克風。不傳輸觸控。無法連線到 Windows 家用版：它無法作為遠端桌面主機。

- 主機透過圖形管線（MS-RDPEGFX）繪製，每個工作階段都把它直通給瀏覽器合成。這幾乎把畫面的所有工作都從閘道上拿走了。它需要 WebGL 2 和跨來源隔離（cross-origin isolated）的頁面；如果代理伺服器丟掉了閘道的兩個隔離回應標頭，就會破壞這一點；不符合條件的瀏覽器不能啟動該目標。主機如何繪製管線，高品質或自適應 H.264（alpha），在選擇器中選擇。
- `egfx = false` 讓主機改用點陣圖更新繪製，由閘道以圖塊傳送：適用於無法合成管線的瀏覽器。這時桌面維持固定尺寸：沒有管線時，Windows 透過重建共享來調整尺寸，而本閘道不接受這種方式。
- **Beta：** `virtual_displays = 2` 要求主機提供兩個顯示器，每個都是工作階段的尺寸。第二個相對於第一個的位置在選擇器中該目標下的 **Second display** 選擇：右、左、上或下，與用戶端自己第二個顯示器的位置對應。工作階段從浮動選單 Display 選擇器中的 *All Displays* 開始，它在此處保留第一個顯示器，並在選單的 Display 按鈕下連結到第二個，即 **Open Display 2 ↗**：它在同一個瀏覽器的新分頁中開啟 `/display/2`，只顯示該顯示器，有自己的指標和鍵盤；再按一次會把那個分頁切到前景。選擇器中的 `Display 1` 和 `Display 2` 在本頁面單獨顯示一個顯示器。在兩個分頁之間拖過邊緣的視窗會出現在另一個顯示器上，指標也一樣，無論它們的視窗如何擺放。當每個分頁都在各自的顯示器上全螢幕時，邊緣相接，視窗會出現在拖到的位置；兩個畫面之間隔著什麼，視窗就差多少。選擇 *All Displays* 時，Display 選擇器和選單的 **Info** 也提供同樣的連結。頁面開啟時就顯示該顯示器。在跟隨視窗的工作階段中，那個分頁的視窗決定顯示器 2 的尺寸。它需要持有工作階段之瀏覽器的登入，而且一次只在一個分頁中顯示：另一個分頁正在顯示它時，新開啟的分頁會提示它正在使用中，並在 **Take over**（接管）之前詢問，就像瀏覽器接管工作階段前會詢問一樣；被接管的分頁會說明這一點並提供 **Take it back**。那個分頁選單中的 **Disconnect** 會中斷它，單獨選擇一個顯示器會關閉它。在它沒有被顯示時開啟，會提示無法使用。啟用管線時，持有工作階段的瀏覽器只合成兩個顯示器一次並顯示其中一個；第二個顯示器的分頁從同一個畫面繪製，因此沒有任何東西被解碼兩次。已在一台 Windows 11 主機上檢驗，其中兩個顯示器的直通只透過無頭瀏覽器的通訊端檢驗過，尚未用肉眼檢查。

  第二個顯示器的分頁有自己的選單，位於一個顯示該顯示器編號 2 的按鈕後面，而工作階段頁面上這個按鈕顯示 1 而不是 ☰。選單裡只有該分頁自己的內容：**Immersive full screen**、該顯示器的尺寸和密度，以及 **Disconnect**；其他所有內容都留在工作階段頁面的選單中。

  第二個顯示器的分頁適用於有兩個實體顯示器的用戶端，每個顯示器顯示一個。只有一個顯示器的用戶端不開啟該分頁，在此處像只有一個顯示器時那樣使用第一個，或者選擇一個顯示器。手機或平板上不支援該分頁，儘管那裡沒有任何東西阻止你開啟它。

### 精確畫面

預設情況下，Windows 主機以有損方式繪製大部分桌面，使用 RemoteFX Progressive 和 ClearCodec 的 NSCodec，會讓文字和顏色稍微模糊。這由主機自己的原則決定，zDesk 中沒有任何設定能覆寫它。要讓它精確繪製每個像素：

1. 在主機上開啟本機群組原則編輯器（`gpedit.msc`），前往「電腦設定 > 系統管理範本 > Windows 元件 > 遠端桌面服務 > 遠端桌面工作階段主機 > 遠端工作階段環境」（Computer Configuration > Administrative Templates > Windows Components > Remote Desktop Services > Remote Desktop Session Host > Remote Session Environment）。啟用 *設定 RemoteFX Adaptive Graphics 的影像品質*（*Configure image quality for RemoteFX Adaptive Graphics*），並設為 **不失真**（**Lossless**）。沒有群組原則編輯器的 Windows 版本，可以在以系統管理員身分執行的命令提示字元中透過登錄進行同樣的設定：

   ```text
   reg add "HKLM\SOFTWARE\Policies\Microsoft\Windows NT\Terminal Services" /v ImageQuality /t REG_DWORD /d 1 /f
   ```

   如果網域的群組原則設定了該項，則以網域的群組原則為準。
2. 執行 `gpupdate /force`，然後啟動新的工作階段：已在執行的工作階段維持它啟動時的品質。
3. 在選擇器中，把 **Graphics** 保持為 *High quality*。這樣主機只使用精確的編碼，即 planar 和未壓縮的矩形，並原封不動地傳給瀏覽器。
4. 在工作階段中開啟選單的 **Info**：*Host's picture* 一列會顯示 *Lossless (Planar)* 或類似內容。*Lossy (…)* 則列出主機改用的編碼，表示原則還沒有套用到該工作階段。

無損的代價很高，因為主機也會精確繪製移動中的內容：光是開啟時的桌面就約 7.6 MB，視窗中的影片約每秒 19 MB，這還是主機自身壓縮之前的數字；該壓縮隨資料一起到達瀏覽器，可讓 planar 繪製減少約三分之一，捲動減少約一半。這適合區域網路。在這樣的主機上，自適應 H.264 不會改變任何東西：Lossless 下它不繪製 H.264。測量資料請見 [Lossless, by the host's policy](rdp-client.md#lossless-by-the-hosts-policy)。

其他 RDP 伺服器，例如較舊的 Windows 或 xrdp，或許剛好能用，但未經測試。請見 [The RDP client](rdp-client.md)。

## Mac (`vnc` 加 Apple 子類型)

```toml
[[targets]]
name = "mac"
protocol = "vnc"
subtype = "ard"   # 或 "ard-high-performance"
host = "192.0.2.11"
username = "andrew"
password_ref = "mac"
```

Mac 不需要安裝額外的軟體。為目標提供 Mac *帳號*的使用者名稱和密碼，而不是螢幕共享密碼：這樣會選擇 Apple Remote Desktop 認證，連線會進入該使用者自己的螢幕，而不是登入視窗。Mac 必須在遠端管理（Remote Management）的個別使用者存取清單中授予該帳號觀察（Observe）和控制（Control）權限；預設的「所有使用者」設定會拒絕連線，回報的錯誤與憑證錯誤相同。請見 [Remote Management access](apple-vnc-889.md#remote-management-access)。

- **`subtype = "ard"`** 是螢幕共享的標準模式：Mac 的實體顯示器，一次一個或全部，依其自身密度顯示。一次一個螢幕的工作階段以選擇器中的 Adaptive（beta，預設）啟動，它直通 Mac 自己的 Adaptive 畫面供頁面解碼；或以 Gateway adaptive 啟動，即閘道針對 Mac 無損 Full 品質編碼的圖塊，`video_quality` 對它有作用。兩者都只在虛擬 Mac 上試過。Combined Display（alpha）工作階段是閘道同時編碼所有螢幕的圖塊。它不支援調整尺寸和聲音；Mac 繼續在自己的輸出裝置上播放。
  - **連接了多個顯示器的 Mac 最好設定 `virtual_display = true`**（請見下文），它把一個顯示器調整為瀏覽器視窗的尺寸。Combined Display 同時顯示所有螢幕，以 100% 顯示，因此只適合兩個並排、能放進一個瀏覽器視窗的小顯示器，再大就得不停捲動。它沒有畫面上限：每個螢幕都完整顯示，無論有多少個、加起來多大，都以瀏覽器密度對應的像素顯示，因此 1x 瀏覽器上的 2x 螢幕仍由閘道縮小（請見上文）。只在虛擬 Mac 上試過。
- 所有 Apple 子類型都傳輸 Mac 原生的剪貼板。
- **`subtype = "ard-high-performance"`** 是 Apple 檢視器中的高效能模式。Mac 會停用其實體顯示器，把所有視窗放到一個由工作階段決定尺寸的虛擬顯示器上，最大 3840×2160 後備像素。畫面和聲音透過 Mac 的媒體串流傳輸，即透過 SRTP 的 HEVC 和 AAC-ELD：
  - **Beta：** `virtual_displays = 2` 要求 Mac 提供兩個虛擬顯示器，即 Apple 檢視器的「2 Virtual Displays」，第二個在第一個右側：位置由 Mac 決定，因此選擇器不提供其他方向。要放到其他方向，請在工作階段開啟期間到 Mac 的「系統設定 → 顯示器 → 排列」中調整：把第二個顯示器拖到第一個的左側、上方或下方。工作階段會跟著變化，每個分頁維持自己的顯示器，指標穿過它們現在共用的那條邊；Mac 可能會為下一次工作階段保留這個排列，沒有保留時它又回到右側。工作階段從 Display 選擇器的 *All Displays* 開始，它在此處保留第一個顯示器，並在同一個瀏覽器的新分頁中開啟 `/display/2` 顯示第二個，與 RDP 目標一樣，選擇器也可以單獨顯示其中任一個；在跟隨視窗的工作階段中，那個分頁的視窗決定第二個顯示器的尺寸。Mac 為每個顯示器傳送一路串流，第二路傳到 UDP 連接埠 5902，閘道只直通正在顯示的顯示器。與 RDP 一樣，在兩個分頁之間拖過邊緣的視窗會出現在另一個顯示器上；不建議在只有一個實體顯示器的用戶端上使用第二個顯示器的分頁，手機或平板上也不支援。
  - Mac 向閘道的 UDP 連接埠 5900 和 5901 傳送資料，有第二個顯示器時還有 5902，因此兩者之間的防火牆或 NAT 必須放行。
  - Linux 閘道的 `net.core.rmem_max` 至少要 4194304，供畫面的通訊端使用，低於該值時記錄檔會發出警告：預設的 212992 在 Retina 尺寸下會遺失關鍵影格。
  - 閘道不解碼 Mac 的任何一路串流：畫面以選擇器中選擇的 HEVC 或 H.264 直通。
  - H.264 直通不需要在頁面中加入任何東西：閘道要求 Mac 用 H.264 取代 HEVC，瀏覽器用自己的解碼器解碼，而能解碼 H.264 的瀏覽器比能解碼 HEVC 的多。因此預設建置不需要軟體解碼器，就能為任何具備 `VideoDecoder` 的瀏覽器（包括 Firefox）提供該目標；選擇器也只在這些瀏覽器中提供 H.264，沒有 `VideoDecoder` 時會變灰。選擇器不詢問解碼器本身：解碼器不接受 Mac 的 H.264 的瀏覽器會在工作階段中透過解碼器的錯誤說明。
  - HEVC 直通只有在頁面啟動時的探測找到解碼路徑時才傳送 HEVC。支援情況取決於瀏覽器、平台和 GPU，因此以選擇器為準。閘道還可以提供 [`zdesk.example.toml`](../zdesk.example.toml) 中描述的選用軟體畫面解碼器。
  - Mac 推送串流時會把自己的喇叭靜音，因此工作階段一律傳輸 AAC-ELD 聲音。頁面會單獨探測它；如果無法解碼聲音，工作階段會在沒有音訊的情況下繼續，並在選單中說明原因。
- **非官方：** 在 `ard` 目標上設定 `virtual_display = true`，會在這樣的虛擬顯示器上以標準模式開啟，支援調整尺寸，但沒有媒體串流和聲音，期間實體顯示器會被停用。這是存取連接了多個顯示器之 Mac 的建議方式，因為超過兩個小螢幕時，Combined Display 在瀏覽器中並不實用。Apple 的檢視器從不提供這種組合；只在 macOS 26 上測試過。

Apple 沒有公開這個協定的任何文件。兩種模式都是逆向工程而來，只在測量過的 Mac 上保證正確，macOS 的更新隨時可能改變其中任何部分。請見 [Apple RFB 003.889, as measured](apple-vnc-889.md)。

## wlshare (`vnc`, `subtype = "wlshare"`)

```toml
[[targets]]
name = "wayland"
protocol = "vnc"
subtype = "wlshare"
host = "192.0.2.12"
username = "me"
password_ref = "wayland"
```

[wlshare](https://github.com/andrewtheguy/wlshare/blob/main/README.zh-TW.md) 是本專案自己的 VNC 伺服器，針對以 wlroots 為基礎的 Wayland 桌面。這個子類型讓閘道列出 wlshare 的私有 RFB 擴充：它的 VP9 串流，原封不動地直通並隨瀏覽器連線自適應；像素密度；輸出清單；聲音；以及在勾選後啟動的工作階段中，作為桌面上 PipeWire 裝置的瀏覽器攝影機和麥克風。不設定子類型時，同一個伺服器會以任何 VNC 伺服器的方式讀取。

- **畫面（Picture）。** 選擇器的 Picture 群組為 *Video, passed through*（預設）：wlshare 針對整個桌面編碼的 VP9，依瀏覽器的連線調整。或 *Tiles with motion, passed through*：靜止的部分為精確的 PNG，每個移動區域有自己的一路視訊串流，使用主機硬體編碼器或 libx264 編碼的 H.264，停下後再精確地重送。圖塊適用於頻寬充裕的連線：wlshare 花在圖塊上的時間只是 VP9 的一小部分，最終畫面是精確的，代價是數倍的位元組數，大面積移動時達每秒數十 Mbit。或 *Progressive tiles (beta), passed through*，適用於頻寬並非一直充裕的連線：每個圖塊，無論靜止還是移動，都是 RemoteFX Progressive，就像 Windows 主機透過 RDP 在沒有 H.264 時繪製其桌面那樣——先送粗略的一遍，靜止後就地細化，移動的部分以相對於頁面所持內容的變更傳送——與 VP9 一樣依連線調整，品質從 `video_quality` 逐步調降，較慢的連線只是影格較少。頁面自己解碼它們，就像它合成 RDP 主機的管線那樣，因此不需要視訊解碼器，在無法合成管線的頁面上會變灰；它永遠達不到無損。`render_adaptive` 只調整 VP9。

- **Beta：** 剛好有兩個輸出（顯示器或無頭輸出）的桌面會列出 *All Displays* 並從它開始：第一個留在此處，第二個在同一個瀏覽器的新分頁中開啟 `/display/2`，有自己的指標和鍵盤，與有兩個虛擬顯示器的 `rdp` 目標一樣。`virtual_displays = 2` 要求 wlshare 提供兩個，就像要求 Windows 主機或 Mac 那樣：在 sway 上，wlshare 會讓合成器建立第二個無頭輸出，以兩個啟動的工作階段在要求一個（預設）時維持一個；在其他合成器上，輸出就是合成器既有的那些。

範例使用 wlshare 執行時所用的帳號，透過 RSA-AES 上的 PAM 驗證。如果 wlshare 改用 `[password]` 表格，請用 `password_ref` 參照該密碼，並省略 `username`，此時 RSA-AES 只傳送密碼。wlshare 不提供傳統 VNC 認證，因此不使用 `vnc_password_ref`。

閘道和 wlshare 必須使用同一個版本的 wlshare 協定；`wlshare --version` 會列出某個 wlshare 所使用的版本。連線到另一個版本的 wlshare 時，工作階段會在連線時失敗，並指明兩個版本；把兩者中較舊的那一個升級即可解決。不變更協定的 wlshare 發行會維持其版本不變。

請見 [密度](wlshare-density.md)、[輸出](wlshare-outputs.md)、[音訊](wlshare-audio.md)、[攝影機](wlshare-camera.md) 和 [麥克風](wlshare-microphone.md)。

## 其他 VNC 伺服器 (`vnc`)

```toml
[[targets]]
name = "workstation-vnc"
protocol = "vnc"
host = "192.0.2.13"
vnc_password_ref = "workstation-vnc"
```

一般目標用 `vnc_password_ref` 進行傳統 VNC 認證，或用 `username` 和 `password_ref` 進行 RSA-AES 認證。只要求密碼的 RSA-AES 伺服器使用 `password_ref`，不設定 `username`。閘道會要求 ZRLE，所有現行的 VNC 伺服器都支援它，此外只讀取 Raw；較舊的標準編碼、Tight 以及其他廠商的或有損的編碼都不在清單中。目標依伺服器自身的尺寸、以 1x 顯示，不傳輸聲音、攝影機或麥克風。標準 RFB 無法表明其像素是 HiDPI；請見 [HiDPI over standard RFB](standard-rfb-hidpi.md)。

## 攝影機和麥克風

**Camera** 和 **Microphone** 是 `rdp` 或 `wlshare` 目標在選擇器中的勾選項，用於向遠端提供這個瀏覽器的攝影機和麥克風。兩者都是**實驗性**的，預設關閉，並像其他選項一樣依目標記住。勾選攝影機啟動的工作階段在連線時就給遠端一台攝影機，這個瀏覽器的攝影機只在遠端有程式使用它時才開啟，其餘時間指示燈不亮；資訊卡片的 Camera 一列會說明目前狀態。麥克風也一樣：工作階段連線時就接上，這個瀏覽器的麥克風只在遠端有程式錄音時才開啟，資訊卡片的 Microphone 一列會說明。它們針對臨時需要的情境，例如一次通話或錄製，並以盡可能低的負擔傳送：瀏覽器的 H.264 不經轉碼直通，以及低固定位元率的單聲道語音 Opus。Apple 目標和一般 `vnc` 目標都不提供這兩項。

在 RDP 上，攝影機需要一台會重新導向攝影機的 Windows 主機：工作站版本的 Windows，或安裝了遠端桌面工作階段主機角色的 Windows Server。Windows 主機只有在上面有程式錄音時才會啟動麥克風。

在 wlshare 上，攝影機是桌面上的一個 PipeWire 攝影機，桌面上的瀏覽器只能透過搭配 `xdg-desktop-portal-gtk` 的 `xdg-desktop-portal` 存取它，wlshare 的套件建議安裝（Recommends）這兩者；Chrome 還需要開啟 `chrome://flags/#enable-webrtc-pipewire-camera`。缺少它們時，要求攝影機的網頁找不到任何攝影機。麥克風兩者都不需要：它是一個一般的 PipeWire 音訊來源。

## 本機執行個體

原生安裝包含一個終端機控制平面，可以在一台電腦上執行多個閘道，每個都有自己的設定和瀏覽器來源（origin）：

```sh
zdesk tui
```

它只監聽回送位址，預設連接埠為 52380，可以用 `--port` 或 `ZDESK_TUI_PORT` 變更；連接埠已被其他程式占用時拒絕啟動。開啟 <http://zdesk.localhost:52380>；每個執行中的執行個體位於 `http://<instance>.zdesk.localhost:52380`。它只回應這些名稱，而且只在自己的連接埠上回應。該連接埠上的 `localhost`、`127.0.0.1` 和 `[::1]` 會被重新導向到 `zdesk.localhost`。其他主機名稱或連接埠一律拒絕，因此以其他主機名稱或連接埠到達的通道或連接埠轉送進不了 TUI；但以該連接埠上的 `zdesk.localhost` 到達、或把主機名稱改寫成它的通道或轉送，仍然可以進入。

| 按鍵 | 操作 |
|---|---|
| `↑`/`↓`、`k`/`j` | 選擇執行個體 |
| `n` | 建立執行個體 |
| `e` | 用 `$VISUAL` 或 `$EDITOR`（`vi`，Windows 上為記事本）編輯其 `zdesk.toml`；編輯會經過檢查，在執行個體重新啟動後生效 |
| `p` | 把密碼加密到其 `[passwords]` 中，會詢問名稱和密碼；沒有 `[master]` 的設定會先得到一個 |
| `s`、`x`、`r` | 啟動、停止、重新啟動執行個體 |
| `a` | 啟動所有已停止的執行個體 |
| `o` | 在瀏覽器中開啟執行中的執行個體 |
| Enter | 顯示其設定 |
| `R` | 重新掃描執行個體目錄 |
| `P` | 重新產生登入 PIN，並重新開始計時 |
| `?` | 列出所有按鍵 |
| `q`、Esc | 停止所有執行個體並離開 |

執行個體根目錄下的每個直接子目錄就是一個執行個體：Linux 上為 `~/.local/share/zdesk/instances`（或 `$XDG_DATA_HOME/zdesk/instances`），macOS 上為 `~/Library/Application Support/zdesk/instances`，Windows 上為 `%LOCALAPPDATA%\zdesk\instances`；可以用 `--instances-dir` 指定其他目錄。根目錄只有你的帳號能存取，因為設定中包含憑證。執行個體的設定與閘道的設定有相同的 `[branding]` 和 `[[targets]]`，但沒有 `[server]` 區塊：TUI 占用連接埠，並把每個子網域代理到該執行個體私有的 Unix 通訊端或具名管道。

參照了加密密碼的設定需要主密碼才能啟動（請見[目標密碼](#目標密碼)）。TUI 從鑰匙圈中取得主密碼，第一次時在那裡產生，然後只把每個 worker 自己設定的金鑰交給它。從另一個帳號的鑰匙圈複製來的執行個體設定在本帳號下無法開啟：刪除其 `[master]` 和 `[passwords]`，再用 `p` 重新設定每個密碼。

瀏覽器以 PIN 登入。TUI 執行個體清單上方的存取面板以獨立的顏色顯示一組六位數 PIN（形如 `123-456`），以及它到期的時刻，也就是產生後十分鐘。PIN 只需輸入一次，直接在瀏覽器開啟的頁面上輸入即可，首頁或執行個體頁面皆可，登入後瀏覽器仍停留在原頁面。這一次登入對首頁和所有執行個體都有效。`P` 重新產生 PIN 並重新開始計時。輸錯五次後 PIN 會被撤銷，在以 `P` 產生新的 PIN 之前，所有輸入都會被拒絕。PIN 只保護新的登入。已經登入的瀏覽器在 PIN 到期或重新產生後、在其執行個體重新啟動後都維持登入。只有結束 TUI 才會讓它登出。

PIN 能擋住看不到 TUI 終端機的人，但有一個缺口：cookie 不區分連接埠，電腦上的其他帳號如果在另一個連接埠以 `zdesk.localhost` 或其下的名稱提供頁面，並讓你開啟指向它的連結，就能取得已登入瀏覽器的登入。不要在與不應存取這些桌面的使用者共用的電腦上執行 `zdesk tui`。私有檔案和 worker 端點可以防止其他本機使用者取得憑證。瀏覽器從不持有 worker 的權杖：TUI 代替已登入的瀏覽器出示它。瀏覽器進入閘道只有兩種方式：這組 PIN，或獨立執行的閘道的使用者名稱和密碼。

請見 [Local multi-instance control plane](architecture.md#local-multi-instance-control-plane)。
