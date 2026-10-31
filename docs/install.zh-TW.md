# 安裝 zDesk

[English](install.md) | [简体中文](install.zh-CN.md) | 繁體中文

## 原生套件

從[最新發行版](https://github.com/andrewtheguy/zdesk/releases/latest)安裝原生套件。套件管理員管理閘道執行檔、設定範例和授權條款；Web 用戶端編譯在執行檔中。它不管理實際使用的設定，因此升級或移除永遠不會取代或刪除憑證。

Linux 發行版二進位檔需要 glibc 2.39 或更新版本：Debian 13、Ubuntu 24.04 或更新的版本，以及具有相同基準的 RPM 發行版。單憑套件格式並不能讓二進位檔相容於較舊的發行版。

### Debian 13+ 和 Ubuntu 24.04+（`.deb`）

發行版提供 `zdesk-linux-amd64.deb` 和 `zdesk-linux-arm64.deb`：

```sh
curl -fsSLO https://github.com/andrewtheguy/zdesk/releases/latest/download/zdesk-linux-amd64.deb
sudo apt install ./zdesk-linux-amd64.deb
```

在 arm64 主機上使用 `arm64` 檔名。套件安裝以下檔案：

```text
/usr/bin/zdesk
/usr/share/doc/zdesk/zdesk.example.toml
/usr/share/doc/zdesk/LICENSE
```

### glibc 2.39+ 的 RPM 發行版（`.rpm`）

發行版提供 `zdesk-linux-amd64.rpm` 和 `zdesk-linux-arm64.rpm`：

```sh
curl -fsSLO https://github.com/andrewtheguy/zdesk/releases/latest/download/zdesk-linux-amd64.rpm
sudo dnf install ./zdesk-linux-amd64.rpm
```

在 arm64 主機上使用 `arm64` 檔名。此套件使用與 `.deb` 相同的 `/usr/bin` 和 `/usr/share` 配置。`sudo rpm -i` 和發行版的其他 RPM 前端也可以使用，但建議使用 `dnf`，因為它會解析相依性。

### macOS（`.pkg`）

閘道套件為 arm64：

```sh
curl -fsSLO https://github.com/andrewtheguy/zdesk/releases/latest/download/zdesk-macos-arm64.pkg
sudo installer -pkg zdesk-macos-arm64.pkg -target /
```

它安裝以下檔案：

```text
/usr/local/bin/zdesk
/usr/local/share/doc/zdesk/zdesk.example.toml
/usr/local/share/doc/zdesk/LICENSE
```

此套件未簽署，也未經公證。透過瀏覽器下載的檔案會被隔離，因此請依上面的方式用 `curl` 取得，並在終端機中安裝。`.pkg` 包含閘道 CLI，Web 用戶端在其中。

### Windows（`.msi`）

Windows x86-64，在以系統管理員身分執行的 PowerShell 7（`pwsh`）中：

```powershell
Invoke-WebRequest https://github.com/andrewtheguy/zdesk/releases/latest/download/zdesk-windows-x86_64.msi -OutFile zdesk-windows-x86_64.msi
msiexec /i zdesk-windows-x86_64.msi
```

此套件未簽署，因此 SmartScreen 會在執行前詢問。它開啟常見的安裝精靈：選擇資料夾、確認，完成後顯示結束頁面。它安裝與 Unix 套件相同的目錄樹，預設位於 `%ProgramFiles%\zdesk`，並把 `bin` 加入系統 `PATH`，因此安裝後開啟的 shell 中可以直接使用 `zdesk`：

```text
C:\Program Files\zdesk\bin\zdesk.exe
C:\Program Files\zdesk\VERSION
C:\Program Files\zdesk\share\doc\zdesk\zdesk.example.toml
C:\Program Files\zdesk\share\doc\zdesk\LICENSE
```

閘道從 `%ProgramData%\zdesk\zdesk.toml` 讀取設定。加上 `/qn` 可進行自動安裝。多執行個體控制平面 `zdesk tui` 也包含在內，它把執行個體儲存在 `%LOCALAPPDATA%\zdesk\instances` 下。

## 首次設定

設定中包含網頁登入和目標的憑證。請以將執行 `zdesk serve` 的帳號建立它，權限為 `0600`。套件只附帶公開的 [`zdesk.example.toml`](../zdesk.example.toml)，用來建立設定。同一帳號也擁有狀態目錄，啟用的 `[meter]` 預設把資料庫儲存在那裡。

在 Linux 上：

```sh
sudo install -d -m 700 -o "$(id -un)" -g "$(id -gn)" /etc/zdesk /var/lib/zdesk
sudo install -m 600 -o "$(id -un)" -g "$(id -gn)" \
  /usr/share/doc/zdesk/zdesk.example.toml /etc/zdesk/zdesk.toml
${EDITOR:-vi} /etc/zdesk/zdesk.toml
```

在 macOS 上：

```sh
sudo install -d -m 700 -o "$(id -un)" -g "$(id -gn)" /usr/local/etc/zdesk /usr/local/var/zdesk
sudo install -m 600 -o "$(id -un)" -g "$(id -gn)" \
  /usr/local/share/doc/zdesk/zdesk.example.toml \
  /usr/local/etc/zdesk/zdesk.toml
${EDITOR:-vi} /usr/local/etc/zdesk/zdesk.toml
```

在 Windows 上，在依預設路徑安裝後開啟的 PowerShell 7（`pwsh`）中執行以下命令，只有執行閘道的帳號可以讀取該目錄，設定以及傳輸量資料庫及其 SQLite 檔案都繼承這項權限。如果在精靈中選擇了其他資料夾，請改用該資料夾下的 `share\doc\zdesk\zdesk.example.toml` 作為 `Copy-Item` 的來源：

```powershell
New-Item -ItemType Directory -Force "$env:ProgramData\zdesk" | Out-Null
icacls "$env:ProgramData\zdesk" /inheritance:r /grant:r "${env:USERNAME}:(OI)(CI)F"
Copy-Item "$env:ProgramFiles\zdesk\share\doc\zdesk\zdesk.example.toml" "$env:ProgramData\zdesk\zdesk.toml"
notepad "$env:ProgramData\zdesk\zdesk.toml"
```

在 `[server].site_username` 中設定網頁登入的使用者名稱，並把範例中的 `[[targets]]` 項目換成要存取的遠端桌面。密碼都不以明文寫入：網頁登入的密碼是 `[server].site_passwd_enc`，目標透過 `password_ref` 參照自己的密碼，設定用你的主密碼將兩者加密儲存，閘道從 `[server].master_password_file` 指定的檔案讀取主密碼。在設定中為它指定設定旁邊的一個檔案（`master_password_file = "master-password"`；相對路徑以設定所在的目錄為基準）。接著 `password master create` 詢問新的主密碼，把它寫入該檔案（該檔案沿用目錄僅限你的帳號存取的權限），並輸出要貼到設定中的 `[master]` 區塊；`password encrypt` 從該檔案讀取主密碼並輸出一個值：一次給網頁登入的密碼，以 `site_passwd_enc = "…"` 的形式貼上；一次給目標的密碼，以 `example = "…"` 的形式貼到 `[passwords]` 下：

```sh
zdesk password master create
zdesk password encrypt   # 網頁登入的密碼
zdesk password encrypt   # 目標的密碼
```

請見[目標密碼](https://zdesk.dev/zh-TW/guide#target-passwords)。在前景啟動閘道：

```sh
zdesk serve
```

然後開啟 <http://localhost:52380>。[使用 zDesk](https://zdesk.dev/zh-TW/guide) 說明各類目標如何設定，以及如何從另一台電腦存取頁面。

## 容器

映像檔發布於 Linux amd64 和 arm64，附有 `latest` 和 `v<version>` 標籤。映像檔中沒有 `zdesk tui`。

與任何提供服務的閘道一樣，主密碼來自一個檔案。把它保存在 secret 中，在設定中指定該檔案，並用同一映像檔寫入設定的密碼，貼上每個命令輸出的內容：網頁登入的密碼貼為 `[server].site_passwd_enc`，每個目標的密碼貼到 `[passwords]` 下：

```toml
[server]
site_username = "admin"
site_passwd_enc = "$ZD;1;AES-256-GCM;…"
master_password_file = "/run/secrets/zdesk_master"
```

```sh
mkdir -m 700 secrets
# 詢問新的主密碼，以 0600 寫入 secrets/zdesk_master
docker run --rm -it -v ./zdesk.toml:/opt/zdesk/etc/zdesk.toml:ro \
  -v ./secrets:/run/secrets \
  ghcr.io/andrewtheguy/zdesk:latest password master create -c /opt/zdesk/etc/zdesk.toml
docker run --rm -it -v ./zdesk.toml:/opt/zdesk/etc/zdesk.toml:ro \
  -v ./secrets:/run/secrets:ro \
  ghcr.io/andrewtheguy/zdesk:latest password encrypt
```

以唯讀方式掛載同一目錄來啟動閘道：

```sh
docker run -d --name zdesk -p 52380:52380 \
  -e ZDESK_LISTEN=0.0.0.0:52380 \
  -v ./zdesk.toml:/opt/zdesk/etc/zdesk.toml:ro \
  -v ./secrets:/run/secrets:ro \
  ghcr.io/andrewtheguy/zdesk:latest
```

設定了 `[meter].enabled` 時，還要在 `/opt/zdesk/var` 掛載一個磁碟區，否則記錄會隨容器一起消失。主密碼檔案也可以是 Docker、Podman 或 Kubernetes 的 secret。使用 Helm 時，輸出的 `[master]` 區塊和各個值屬於 chart 用來產生設定的 values；它們是密文，因此只有主密碼需要作為 secret。

## 升級

下載新的套件，並交給同一個套件管理員：

```sh
sudo apt install ./zdesk-linux-amd64.deb
sudo dnf upgrade ./zdesk-linux-amd64.rpm
sudo installer -pkg zdesk-macos-arm64.pkg -target /
msiexec /i zdesk-windows-x86_64.msi
```

只使用對應主機平台的命令。套件檔案會被原地取代。實際使用的設定不在任何套件清單中，因此保持不變。

## 解除安裝

在 Debian 或 Ubuntu 上：

```sh
sudo apt remove zdesk
```

在 RPM 發行版上：

```sh
sudo dnf remove zdesk
```

macOS 上沒有可以呼叫的套件管理員，因此儲存庫提供了一個解除安裝指令碼，它讀取已安裝的收據，並精確移除套件寫入的內容：

```sh
curl -fsSLO https://raw.githubusercontent.com/andrewtheguy/zdesk/main/packaging/uninstall-macos-pkg.sh
sudo bash uninstall-macos-pkg.sh
```

`--dry-run` 只列出要移除的內容而不實際移除，不需要 `sudo`。指令碼在移除後會註銷收據，並保留仍有維運人員放入檔案的任何安裝目錄。手動解除安裝的步驟相同，對應目前的配置：

```sh
sudo rm -f /usr/local/bin/zdesk
sudo rm -rf /usr/local/share/doc/zdesk
sudo pkgutil --forget com.andrewtheguy.zdesk.gateway
```

在 Windows 上，從**應用程式與功能**中移除 zDesk。

以上操作都不會觸及實際使用的設定或狀態。只有在需要一併刪除憑證、設定和傳輸量記錄時，才另行移除 Linux 上的 `/etc/zdesk` 和 `/var/lib/zdesk`、macOS 上的 `/usr/local/etc/zdesk` 和 `/usr/local/var/zdesk`，或 Windows 上的 `%ProgramData%\zdesk`。

## 建置發行套件

先建置 tarball 輸入，再為目前的主機建置原生套件：

```sh
bash packaging/build-tarball.sh
bash packaging/build-native-packages.sh
```

Linux 上建置 `.deb` 和 `.rpm`；macOS 上建置 `.pkg`。發行流程請見 [`packaging/README.md`](../packaging/README.md)（英文）。
