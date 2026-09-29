# 安装 Monica CLI 1.0.101 / Installation

下载：[GitHub Release v1.0.101](https://github.com/Monica-Pass/Monica-cli/releases/tag/v1.0.101)。选择系统对应的 **x86_64** 包，同时下载 `SHA256SUMS`。无需安装 Rust；源码构建另见 `source-checkout.md`。

## Windows 10/11 x64

用 PowerShell 核对 ZIP 的 SHA256，与 `SHA256SUMS` 对应行比较：

```powershell
Get-FileHash .\monica-cli-1.0.101-windows-x86_64.zip -Algorithm SHA256
Expand-Archive .\monica-cli-1.0.101-windows-x86_64.zip -DestinationPath .\MonicaCLI
cd .\MonicaCLI\monica-cli-1.0.101-windows-x86_64
.\monica.exe --version
.\monica.exe
```

目录中同时提供 `monica.exe` 和 `monica-pass.exe`。便携标记文件 `monica-pass.portable` 使数据保存在程序旁边的 `data/`；不要将目录放在没有写权限的位置。程序尚未代码签名。

如需安装到当前用户并加入 PATH，在解压目录执行：

```powershell
.\scripts\install.ps1 -InstallDir D:\Apps\MonicaCLI
```

安装目录必须为绝对路径。脚本会创建用户快捷方式与命令入口；`-NoPath -NoShortcut` 可跳过它们。完成后打开新终端运行 `monica`。不会修改 Codex 或 Claude Code 的配置。

## Linux x86_64

需要 glibc 2.35+。包面向 Ubuntu 22.04+、Debian 12+ 等发行版；不适用于 Alpine/musl。

```sh
sha256sum --ignore-missing -c SHA256SUMS
tar -xzf monica-cli-1.0.101-linux-x86_64.tar.gz
cd monica-cli-1.0.101-linux-x86_64
./monica --version
./monica
```

直接在解压目录运行时使用旁边的 `data/`。如只把 `monica-pass` 可执行文件复制到 PATH 中（例如 `~/.local/bin/monica`），不复制便携标记，则使用 `$XDG_STATE_HOME/monica-pass` 或默认 `~/.local/state/monica-pass`。

Wayland/X11 公钥复制分别需要 `wl-copy` 或 `xclip` / `xsel`。可选的 WebDAV 密码记忆需要 `secret-tool`；可信 stdin 注入的密码不会保存到系统凭据存储。这些工具不是浏览保险库或直连配置的前置依赖。

## 升级与卸载

先关闭 Monica 并备份保险库。保留整个便携 `data/`、保险库关联的 `.blobs` 与其他数据库附属文件；不要用旧包中的数据覆盖现有数据。将新包解压到单独目录，再使用安装脚本更新现有的便携安装，或在关闭程序后替换程序和文档文件。

卸载程序不会在上游撤销 Key。直连模式写出的客户端配置与 `.monica-*.bak` 备份需单独管理；停止使用时在服务商处撤销/轮换 Key。只删除程序目录中的可执行文件不会删除存放在别处的保险库。

## English quick start

Download the x86_64 Windows ZIP or Linux tar.gz and verify it against `SHA256SUMS`. Extract into a writable folder and run `monica.exe` (Windows) or `./monica` (Linux). The portable marker stores local data in the adjacent `data/` folder. Keep that folder and all vault sidecars when upgrading. Windows requires Windows 10 or newer; Linux requires glibc 2.35 or newer. No Rust installation is required for binary packages.

The Windows script optionally installs for the current user and adds PATH/shortcuts. The Linux executable can also be copied into PATH without the portable marker to use the XDG state directory. See `docs/direct-config.md` and `docs/model-proxy.md` for model clients. Direct configurations contain the raw upstream Key and are not controlled by Monica grant limits.
