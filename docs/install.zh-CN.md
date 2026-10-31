# 安装 zDesk

[English](install.md) | 简体中文 | [繁體中文](install.zh-TW.md)

## 原生软件包

从[最新发布](https://github.com/andrewtheguy/zdesk/releases/latest)安装原生软件包。软件包管理器管理网关可执行文件、配置示例和许可证；Web 客户端编译在可执行文件中。它不管理实际使用的配置，因此升级或卸载永远不会替换或删除凭据。

Linux 发布版二进制文件需要 glibc 2.39 或更高版本：Debian 13、Ubuntu 24.04 或更新的版本，以及具有相同基线的 RPM 发行版。仅凭软件包格式并不能让二进制文件兼容更旧的发行版。

### Debian 13+ 和 Ubuntu 24.04+（`.deb`）

发布中提供 `zdesk-linux-amd64.deb` 和 `zdesk-linux-arm64.deb`：

```sh
curl -fsSLO https://github.com/andrewtheguy/zdesk/releases/latest/download/zdesk-linux-amd64.deb
sudo apt install ./zdesk-linux-amd64.deb
```

在 arm64 主机上使用 `arm64` 文件名。软件包安装以下文件：

```text
/usr/bin/zdesk
/usr/share/doc/zdesk/zdesk.example.toml
/usr/share/doc/zdesk/LICENSE
```

### glibc 2.39+ 的 RPM 发行版（`.rpm`）

发布中提供 `zdesk-linux-amd64.rpm` 和 `zdesk-linux-arm64.rpm`：

```sh
curl -fsSLO https://github.com/andrewtheguy/zdesk/releases/latest/download/zdesk-linux-amd64.rpm
sudo dnf install ./zdesk-linux-amd64.rpm
```

在 arm64 主机上使用 `arm64` 文件名。该软件包使用与 `.deb` 相同的 `/usr/bin` 和 `/usr/share` 布局。`sudo rpm -i` 和发行版的其他 RPM 前端也可以使用，但推荐 `dnf`，因为它会解析依赖。

### macOS（`.pkg`）

网关软件包为 arm64：

```sh
curl -fsSLO https://github.com/andrewtheguy/zdesk/releases/latest/download/zdesk-macos-arm64.pkg
sudo installer -pkg zdesk-macos-arm64.pkg -target /
```

它安装以下文件：

```text
/usr/local/bin/zdesk
/usr/local/share/doc/zdesk/zdesk.example.toml
/usr/local/share/doc/zdesk/LICENSE
```

该软件包未签名，也未经公证。通过浏览器下载的文件会被隔离，因此请按上面的方式用 `curl` 获取，并在终端中安装。`.pkg` 包含网关 CLI，Web 客户端在其中。

### Windows（`.msi`）

Windows x86-64，在以管理员身份运行的 PowerShell 7（`pwsh`）中：

```powershell
Invoke-WebRequest https://github.com/andrewtheguy/zdesk/releases/latest/download/zdesk-windows-x86_64.msi -OutFile zdesk-windows-x86_64.msi
msiexec /i zdesk-windows-x86_64.msi
```

该软件包未签名，因此 SmartScreen 会在运行前询问。它打开常见的安装向导：选择文件夹、确认，完成后显示结束页面。它安装与 Unix 软件包相同的目录树，默认位于 `%ProgramFiles%\zdesk`，并把 `bin` 加入系统 `PATH`，因此安装后打开的 shell 中可以直接使用 `zdesk`：

```text
C:\Program Files\zdesk\bin\zdesk.exe
C:\Program Files\zdesk\VERSION
C:\Program Files\zdesk\share\doc\zdesk\zdesk.example.toml
C:\Program Files\zdesk\share\doc\zdesk\LICENSE
```

网关从 `%ProgramData%\zdesk\zdesk.toml` 读取配置。添加 `/qn` 可进行无人值守安装。多实例控制面 `zdesk tui` 也包含在内，它把实例保存在 `%LOCALAPPDATA%\zdesk\instances` 下。

## 首次配置

配置中包含网页登录和目标的凭据。请以将运行 `zdesk serve` 的账户创建它，权限为 `0600`。软件包只附带公开的 [`zdesk.example.toml`](../zdesk.example.toml)，用来创建配置。同一账户也拥有状态目录，启用的 `[meter]` 默认把数据库保存在那里。

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

在 Windows 上，在按默认路径安装后打开的 PowerShell 7（`pwsh`）中运行以下命令，只有运行网关的账户可以读取该目录，配置以及吞吐量数据库及其 SQLite 文件都继承这一权限。如果在向导中选择了其他文件夹，请改用该文件夹下的 `share\doc\zdesk\zdesk.example.toml` 作为 `Copy-Item` 的源：

```powershell
New-Item -ItemType Directory -Force "$env:ProgramData\zdesk" | Out-Null
icacls "$env:ProgramData\zdesk" /inheritance:r /grant:r "${env:USERNAME}:(OI)(CI)F"
Copy-Item "$env:ProgramFiles\zdesk\share\doc\zdesk\zdesk.example.toml" "$env:ProgramData\zdesk\zdesk.toml"
notepad "$env:ProgramData\zdesk\zdesk.toml"
```

在 `[server].site_username` 中设置网页登录的用户名，并把示例中的 `[[targets]]` 条目替换为要访问的远程桌面。密码都不以明文写入：网页登录的密码是 `[server].site_passwd`，目标通过 `password_ref` 引用自己的密码，配置用你的主密码将两者加密保存，网关从 `[server].master_password_file` 指定的文件中读取主密码。在配置中为它指定配置旁边的一个文件（`master_password_file = "master-password"`；相对路径以配置所在目录为基准）。然后 `password master create` 询问新的主密码，把它写入该文件（该文件沿用目录仅限你的账户访问的权限），并输出要粘贴到配置中的 `[master]` 块；`password encrypt` 从该文件读取主密码并输出一个值：一次为网页登录的密码，以 `site_passwd = "…"` 的形式粘贴；一次为目标的密码，以 `example = "…"` 的形式粘贴到 `[passwords]` 下：

```sh
zdesk password master create
zdesk password encrypt   # 网页登录的密码
zdesk password encrypt   # 目标的密码
```

见[目标密码](https://zdesk.dev/zh-CN/guide#target-passwords)。在前台启动网关：

```sh
zdesk serve
```

然后打开 <http://localhost:52380>。[使用 zDesk](https://zdesk.dev/zh-CN/guide) 介绍各类目标如何设置，以及如何从另一台计算机访问页面。

## 容器

镜像发布于 Linux amd64 和 arm64，带有 `latest` 和 `v<version>` 标签。镜像中没有 `zdesk tui`。

与任何提供服务的网关一样，主密码来自一个文件。把它保存在 secret 中，在配置中指定该文件，并用同一镜像写入配置的密码，粘贴每条命令输出的内容：网页登录的密码粘贴为 `[server].site_passwd`，每个目标的密码粘贴到 `[passwords]` 下：

```toml
[server]
site_username = "admin"
site_passwd = "$ZD;1;AES-256-GCM;…"
master_password_file = "/run/secrets/zdesk_master"
```

```sh
mkdir -m 700 secrets
# 询问新的主密码，以 0600 写入 secrets/zdesk_master
docker run --rm -it -v ./zdesk.toml:/opt/zdesk/etc/zdesk.toml:ro \
  -v ./secrets:/run/secrets \
  ghcr.io/andrewtheguy/zdesk:latest password master create -c /opt/zdesk/etc/zdesk.toml
docker run --rm -it -v ./zdesk.toml:/opt/zdesk/etc/zdesk.toml:ro \
  -v ./secrets:/run/secrets:ro \
  ghcr.io/andrewtheguy/zdesk:latest password encrypt
```

以只读方式挂载同一目录来启动网关：

```sh
docker run -d --name zdesk -p 52380:52380 \
  -e ZDESK_LISTEN=0.0.0.0:52380 \
  -v ./zdesk.toml:/opt/zdesk/etc/zdesk.toml:ro \
  -v ./secrets:/run/secrets:ro \
  ghcr.io/andrewtheguy/zdesk:latest
```

设置了 `[meter].enabled` 时，还要在 `/opt/zdesk/var` 挂载一个卷，否则记录会随容器一起消失。主密码文件也可以是 Docker、Podman 或 Kubernetes 的 secret。使用 Helm 时，输出的 `[master]` 块和各个值属于 chart 用来渲染配置的 values；它们是密文，因此只有主密码需要作为 secret。

## 升级

下载新的软件包，并交给同一个软件包管理器：

```sh
sudo apt install ./zdesk-linux-amd64.deb
sudo dnf upgrade ./zdesk-linux-amd64.rpm
sudo installer -pkg zdesk-macos-arm64.pkg -target /
msiexec /i zdesk-windows-x86_64.msi
```

只使用对应主机平台的命令。软件包文件会被原地替换。实际使用的配置不在任何软件包清单中，因此保持不变。

## 卸载

在 Debian 或 Ubuntu 上：

```sh
sudo apt remove zdesk
```

在 RPM 发行版上：

```sh
sudo dnf remove zdesk
```

macOS 上没有可以调用的软件包管理器，因此仓库提供了一个卸载脚本，它读取已安装的回执，并准确删除软件包写入的内容：

```sh
curl -fsSLO https://raw.githubusercontent.com/andrewtheguy/zdesk/main/packaging/uninstall-macos-pkg.sh
sudo bash uninstall-macos-pkg.sh
```

`--dry-run` 只列出要删除的内容而不实际删除，无需 `sudo`。脚本在删除后会注销回执，并保留仍有运维者放入文件的任何安装目录。手动卸载的步骤相同，对应当前的布局：

```sh
sudo rm -f /usr/local/bin/zdesk
sudo rm -rf /usr/local/share/doc/zdesk
sudo pkgutil --forget com.andrewtheguy.zdesk.gateway
```

在 Windows 上，从**应用和功能**中删除 zDesk。

以上操作都不会触及实际使用的配置或状态。只有在需要一并删除凭据、配置和吞吐量记录时，才另行删除 Linux 上的 `/etc/zdesk` 和 `/var/lib/zdesk`、macOS 上的 `/usr/local/etc/zdesk` 和 `/usr/local/var/zdesk`，或 Windows 上的 `%ProgramData%\zdesk`。

## 构建发布软件包

先构建 tarball 输入，再为当前主机构建原生软件包：

```sh
bash packaging/build-tarball.sh
bash packaging/build-native-packages.sh
```

Linux 上构建 `.deb` 和 `.rpm`；macOS 上构建 `.pkg`。发布流程见 [`packaging/README.md`](../packaging/README.md)（英文）。
