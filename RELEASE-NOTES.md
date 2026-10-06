# Monica CLI 1.0.101

首个提供 Linux 和 Windows 安装包的正式版本。

## 功能

- MDBX3 本地保险库、数据库切换、嵌套分类和终端管理；支持与 Monica Android 使用相同的 API Key 和密钥条目格式。
- GitHub / GitLab MCP 凭据网关：按连接、仓库和操作授权，支持到期、调用额度、撤销及人工审批。
- OpenAI / Anthropic 本地模型中转，支持 JSON 与 SSE。客户端使用可撤销本地 Key，真实 Key 留在保险库内；可显式批准受 Tiga 绝对期限约束的模型会话。
- Codex / Claude Code 直连配置：手动输入 Key 或在主密码和 Tiga 导出授权后读取已绑定 Key；保留已有配置并在修改前备份。
- SSH / OpenPGP 密钥管理、WebDAV 保险库同步、结构化 JSON 输出、真实命令语法发现及中英文帮助。

## 下载与安装

- `monica-cli-1.0.101-windows-x86_64.zip`：Windows 10/11 x64，包含可直接运行的程序；静态链接 MSVC CRT，不要求安装 Rust。
- `monica-cli-1.0.101-linux-x86_64.tar.gz`：Linux x86_64，glibc 2.35 或更新版本（如 Ubuntu 22.04+、Debian 12+）。Alpine/musl 不适用。
- `monica-cli-1.0.101-source.tar.gz`：对应 CLI、固定 MDBX 提交和锁定的第三方依赖源码。安装 Rust 1.97 与 C 工具链后可离线构建。
- `SHA256SUMS`：下载校验值。压缩包内包含 `INSTALL.md`、许可证、第三方声明和 `BUILD-INFO.json`。

解压到可写的独立目录，运行 `monica` / `monica.exe`；首次运行可创建或打开保险库。便携包的数据保存在旁边的 `data/`。更新时关闭 Monica，保留整个 `data/` 及保险库附属文件。完整说明见 [安装与升级](docs/release-install.md)。

## 验证与边界

Linux 和 Windows 包均经过隔离构建、CLI 回归、Clippy、命令文档一致性检查，以及解包后启动验证。固定引擎提交为 `f81d3f1f08589670e534473ab8e0030b796dfdf4`。WSL homoos 中的 Codex 0.152.1 和 Claude Code 2.1.258 已使用生成的直连配置，完成第三方 `glm-5.3-200k` 请求。

- 此次提供 x86_64 两平台包；没有 ARM64 或 macOS 包。
- 模型中转支持列模型、Responses、Chat Completions、Messages、count_tokens；不做协议互转、模型映射、故障转移、WebSocket、后台 Responses 或图像/音频/文件专用接口。
- 直连文件包含原始 Key；Monica grant 的期限、撤销、次数和锁库不控制直连请求。需在上游轮换/撤销 Key，并妥善保管客户端文件与备份。
- Android 回写夹具测试因缺少独立 Android 产物仍为忽略；不宣称所有第三方模型或 Android 往返场景均已验证。
- Windows 包尚未进行代码签名；校验值用于核对下载完整性。此版本没有改动 Android、F-Droid 或 MDBX 数据库模型。
