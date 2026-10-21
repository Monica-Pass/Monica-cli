# Monica CLI 未发布改动

## 中文

- 增加代码质量、工作流语法和每周 RustSec 依赖安全检查，固定 GitHub Actions 修订并启用更新提醒。Windows/Linux 打包使用串行 release 测试，仅缓存下载依赖，验证解压和安装后的浏览器辅助程序诊断及 Native Messaging 握手。更新 rustls、rustls-webpki、webpki-roots 与 quinn-proto 的安全补丁。

- 新增独立的 `monica-browser-host` 和当前用户 Windows Edge/Chrome 注册脚本，支持 Monica 浏览器扩展图形化管理 CLI 数据库、目录、Android 兼容密码、GitHub/GitLab 连接，以及查看和撤销 AI 授权。辅助程序使用 CLI 原有解锁、Tiga 和管理函数，不扩展 MCP/AI 命令发现，不返回已有秘密值。支持闲置和绝对期限锁定、原生 head 冲突保护，Token 替换撤销旧授权。
- CLI 发行包和 Windows 安装脚本包含配套浏览器辅助程序。安装后仍需显式注册对应扩展 ID；原数据库和既有发行历史保留。
- 浏览器协议升级为 2，先校验版本/能力再解锁。写入使用持久化操作回执，断开后查询状态，不重发未知结果；新增只读安装诊断及独立 Edge/Chrome manifest。
- 浏览器支持最多 32 KiB 密钥文件、SSH 生成与 SSH/OpenPGP 导入、已有 WebDAV 同步、明确权限的 AI 授权创建/续期。敏感操作重新认证，保留原生身份、Tiga、冲突检查及失焦锁定，已有秘密不返回浏览器；联动仍默认关闭并标记不完善。

## English

- Add code quality, workflow lint and weekly RustSec dependency checks, pin GitHub Actions revisions and enable update PRs. Windows/Linux packaging uses serial release tests and caches downloaded dependencies only; verify native-host diagnostics and Native Messaging hello after extraction/installation. Update security patches for rustls, rustls-webpki, webpki-roots and quinn-proto.

- Add the separate `monica-browser-host` and per-user Windows Edge/Chrome registration script. The Monica extension can graphically manage CLI databases, folders, Android-compatible passwords, GitHub/GitLab connections and AI grant revocation. The helper reuses CLI unlock, Tiga and human management boundaries, exposes no existing secret values or MCP/discovery tools, locks on idle/absolute deadlines and rejects stale password edits. Token replacement revokes old grants.
- Include the matching helper in CLI packages and the Windows installer. Explicit extension-ID registration remains required.
- Negotiate protocol 2, version and capabilities before unlock. Durable operation receipts reconcile disconnected writes without replay. Add read-only installation diagnostics and separate Edge/Chrome manifests.
- Support key files up to 32 KiB, SSH generation and SSH/OpenPGP import, configured WebDAV sync and explicit AI grant creation/renewal with fresh authentication. Preserve native identity, Tiga, conflict checks and focus-loss locking without returning existing secrets. Integration remains off by default and marked incomplete.
