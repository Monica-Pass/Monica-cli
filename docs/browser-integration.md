# 浏览器扩展联动 / Browser integration

Monica 扩展的「侧栏 → CLI 内容管理」通过 Native Messaging 操作本机 CLI 数据库。数据库、主密码校验、Tiga 策略、原生对象身份和写入均由 CLI 的 MDBX3 运行时负责。无需启动 Gateway broker；管理请求会先等待 broker 排空并锁定。

## Windows 安装

使用同一版本构建的 `monica-pass.exe`、`monica-browser-host.exe` 和 `scripts/install-browser.ps1`。源码构建运行 `cargo build --release --locked`；本工作区使用 `cargo +1.97.0-x86_64-pc-windows-gnu build --release --locked`。普通安装脚本会一起安装辅助程序和联动脚本，但不会自动授权扩展。

CLI 联动默认关闭，且标记为「不完善」。先在 CLI 中创建或打开数据库，在扩展「设置与备份 → 通用」启用 CLI 联动，再从侧栏打开 CLI 内容管理。展开「安装浏览器联动」，复制该安装的扩展 ID。在 CLI 安装目录执行：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\install-browser.ps1 -EdgeExtensionId <扩展ID>
```

Chrome 使用 `-ChromeExtensionId`。Firefox 153+ 桌面版使用固定 ID，可与 Edge/Chrome 同时注册：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\install-browser.ps1 -FirefoxExtensionId 'monica@monica-pass.github.io'
```

源码运行时加 `-HostPath .\target\release\monica-browser-host.exe`。注册只影响当前 Windows 用户。Edge/Chrome 精确限定提供的扩展 origin；Firefox 只允许 `monica@monica-pass.github.io`，其他 ID 被拒绝。浏览器安装、解压路径或扩展 ID 改变后需要重新注册。

在同一命令后加 `-Diagnose` 可运行只读诊断：检查辅助程序协议/能力、CLI 是否初始化、注册是否存在、manifest/辅助程序是否丢失及扩展 ID 是否匹配，不读取数据库秘密，也不修复或更改注册。Edge、Chrome、Firefox 使用独立 manifest，单独更新一个浏览器不会改变其他注册。Firefox 使用 `allowed_extensions`，注册位于 `HKCU:\Software\Mozilla\NativeMessagingHosts\app.monica.cli`。辅助程序也可单独运行 `monica-browser-host --diagnose`，输出支持的固定 Firefox ID。

辅助程序与 CLI 放在同一目录时共享默认配置，包括 `monica-pass.portable` 指定的便携 `data/gateway.json`。扩展不能指定任意配置或数据库文件路径。CLI 使用自定义 `-C` 配置时，建议将该实例作为单独的便携安装使用；不会自动枚举其他目录。

卸载联动：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\install-browser.ps1 -Uninstall
```

不提供 ID 时撤销全部三个浏览器的注册；提供一个或多个浏览器 ID 时只撤销对应浏览器，例如 `-FirefoxExtensionId 'monica@monica-pass.github.io' -Uninstall` 保留 Edge/Chrome。数据库、manifest 与 CLI 文件保留。当前附带的安装脚本支持 Windows Edge/Chrome/Firefox。其他系统需要按各浏览器的 Native Messaging 规则手动安装 manifest；Firefox 使用上述固定 `allowed_extensions`，Chromium 使用精确 origin。未完成其他系统的浏览器验收。

Firefox 启动辅助程序时传入 manifest 的绝对路径、固定 add-on ID 两个位置参数；辅助程序拒绝其他 ID、相对路径、混合 Chromium/Firefox 参数和多余参数。manifest 路径只是启动上下文，不读取为 CLI 配置，不选择数据库或提供文件访问能力。Chromium 原有的 `chrome-extension://<32位a-p ID>/` 校验保留。该启动校验不能隔离同一系统用户的任意本地程序。

CLI 参数、注册与隔离测试见 [Firefox 验证记录](firefox-native-messaging-verification-20261021.md)；真实浏览器界面验收由扩展仓库另行记录。

## 可管理内容

- 查看和切换 CLI 已保存的数据库；切换需要目标库密码，会撤销旧 AI 授权，保留原数据库。
- 浏览原生目录和条目，搜索标题、类型和连接名，创建、重命名、移动和删除目录；非空目录和 Android 默认目录保留引擎保护。
- 新增 Android 兼容密码，修改标题和明确勾选的用户名、网址、密码、备注字段。未勾选字段、未知字段、精确数值、原生身份和附件继续由现有 Adapter 保留。编辑核对原生 head，发生并发变化时拒绝覆盖。
- 创建 GitHub/GitLab 默认服务地址的连接、重命名连接、替换 Token。替换 Token 撤销原 AI 授权。
- 查看连接和 AI 授权的公开元数据，明确确认后撤销授权。创建授权时明确选择连接、精确仓库、操作、有效期、请求/调用上限及人工批准策略，默认只读；通用 API 操作需显式 `*` 服务范围。续期保留全部原权限和限制并轮换客户端凭据，原 MCP 客户端需重启。
- 查看 SSH/GPG 的标题、类型、算法和指纹，生成 SSH 密钥或导入本人粘贴的 SSH/OpenPGP 内容。密钥文本限 32 KiB；不返回已存私钥、备注、用户 ID、公钥或导出文件。
- 使用已经在 CLI 配置的 WebDAV 同步；保留 ETag、冲突和原库身份检查。浏览器不能指定新远程地址或本机输出路径；WebDAV 密码仅本次使用，同步完成后锁定。
- 支持解锁时选择二进制密钥文件（最多 32 KiB，更大的文件继续使用 CLI）。先选择文件再输入密码，因为系统文件选择器会触发失焦锁定。密钥操作、同步、授权创建/续期都需再次输入主密码，并向 CLI 重新提交本会话选择的密钥文件；会话锁定后不再保留文件引用。
- 删除会写入原生墓碑，并由后续同步传播。删除绑定连接的条目同时撤销其授权。未知/未来对象保持只读，含附件对象的移动继续受现有保护限制。

已有秘密值不会返回浏览器。编辑表单是指定字段替换，不将“未读取”当作空值覆盖。本人查看完整值继续使用 CLI/TUI 的临时授权详情，不新增通用 JSON、MCP 或标准输出秘密读取入口。

## 通信和锁定

辅助程序是独立的人类管理程序，不属于 CLI AI 命令发现或 MCP。没有 HTTP 端口、网页 CORS 接口、任意命令执行、原始 payload 读取或数据库导出操作。请求使用 Chromium/Firefox Native Messaging 的 4 字节小端长度前缀，严格 UTF-8 JSON 和封闭操作枚举；最大请求 64 KiB，最大回复 900 KiB。过大的库会明确报错，不截断列表。

主密码只通过 Native Messaging stdin 传入辅助程序，保留在 zeroize 包装的进程内存中，不写入浏览器存储、CLI 配置或日志。每个操作重新打开并认证库，核对当前配置路径和原生 vault ID。离开管理页、切换浏览器标签或关闭页面会断开通道；辅助程序在 5 分钟无请求或 15 分钟绝对期限后清除密码。主动锁定不影响数据库内永久保存的凭据。

超时/断开时不自动重试写入。每次写入前浏览器只持久化随机操作 ID、库 ID/路径、操作类型与时间，不保存可执行请求、标题或秘密。辅助程序在调度前私有原子写入并同步 intent，持有跨进程独占锁，完成后记录成功回执。重新解锁对应库后自动查询回执并刷新；成功即可清除提示，未确认或未找到回执则暂停该库的新写入，直到本人检查内容并明确确认。没有回执不能证明没有写入，关闭页面也不能保证中止已提交的写入。

同一个 ID 与库/操作类型只能消费一次，即使载荷改变也不会重执行；不同库或操作类型拒绝复用。未完成 intent 在进程重启后仍为未知结果。日志为配置旁的 `gateway.browser-receipts.json`，仅含公开操作身份、完成标记和固定错误码，不保存秘密或原始响应。日志最多 4096 条且不淘汰旧 ID，达到上限后拒绝新写入（继续使用 CLI 管理）；浏览器最多保留 64 条未确认记录。不要删除日志来重试旧请求。

当前协议为 2，扩展要求 CLI ≥ 1.0.101、integration revision ≥ 2 及 `snapshots`/`mutation_receipts` 能力。`hello` 在解锁前完成协商；密钥文件、密钥、授权和同步按钮按能力显示。除 `hello` 外的请求带 `protocol:2`，写入带 `operation_id`；查询使用 `{op:"operation_status",operation:<UUID>}`，仅已认证的对应原生库可查询，返回 `succeeded`、`unconfirmed` 或 `not_recorded`。更新时须一起替换 CLI、辅助程序和安装脚本。

## English

Open **Sidebar → CLI content management** in the extension. Install the matching `monica-browser-host` beside the CLI, initialize/open a database in the CLI, and register your exact extension ID with `scripts/install-browser.ps1 -EdgeExtensionId <ID>` (or `-ChromeExtensionId`). The setup section shows your ID and command. Run `-Uninstall` to unregister without deleting databases.

The GUI manages saved databases, folders, selective password replacement, connections, grant creation/renewal/revocation, SSH generation and SSH/OpenPGP import, and sync with the CLI's already configured WebDAV. Existing secrets are never returned. Key/grant/sync writes require fresh password authentication plus the key file selected for that session. File/key text is bounded to 32 KiB. Unsupported objects stay read-only. Integration remains off by default and marked incomplete.

The helper negotiates protocol 2, CLI version ≥ 1.0.101 and integration revision ≥ 2 before unlock. The installer supports read-only `-Diagnose` and independent Edge/Chrome manifests. Mutations persist an intent before execution and a public receipt afterward. Reconnect queries receipts without replaying requests. Unknown outcomes block new writes to that database until a person checks its contents and explicitly acknowledges them. The private journal retains up to 4096 consumed IDs without eviction; the browser retains up to 64 pending identities, never executable payloads or secrets. A missing receipt does not prove no write occurred.

Native Messaging uses a separate binary and the existing human management/runtime functions; it adds no MCP tool, HTTP listener, generic secret reader, shell operation or export endpoint. The password stays in zeroized process memory. Leaving the page or switching tabs disconnects the session; idle and absolute limits are 5 and 15 minutes. Stale native heads cannot overwrite newer password edits. Writes are never retried automatically after an unknown outcome.

The provided installer targets Windows Edge/Chrome and desktop Firefox 153+. Register Firefox with `-FirefoxExtensionId 'monica@monica-pass.github.io'`; no other Firefox ID is accepted. Its independent manifest contains only `allowed_extensions`, registered under `HKCU:\Software\Mozilla\NativeMessagingHosts\app.monica.cli`. Firefox passes an absolute manifest path followed by that exact add-on ID; this launch path never selects CLI configuration. Chromium origin validation is unchanged. `-Diagnose` reports all three browsers; `-Uninstall` with IDs removes only selected registrations, while no IDs removes all. Other platforms require manual browser-specific host registration and have not received browser acceptance testing.
