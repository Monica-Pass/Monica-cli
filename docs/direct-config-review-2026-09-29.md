# 直连配置复核（2026-09-29）

## 范围

用户明确要求增加可选的原始 Key 直连方式，并确认同时支持手动输入与读取 Monica 已保存的 Key。新增 `direct-config manual` / `direct-config saved`，生成 Codex TOML 或 Claude Code JSON 配置。CLI 文档、命令发现、练习本及文档站五种语言同步更新。

本轮只修改 CLI 和文档站；没有修改 MDBX 引擎、数据库模型、Android 或 F-Droid。测试采用已发布的 MDBX `f81d3f1f08589670e534473ab8e0030b796dfdf4` 源码快照，不包含共享工作目录的未提交修改。

## 凭据与配置边界

- 手动 Key 只从隐藏提示或可信 stdin `token` 字段输入，不接受 Key 参数或环境变量。
- 保存模式需要主密码、匹配的 API 协议、绑定的原条目版本与端点，以及 Tiga `ExportData` 和 `RevealSecret` 权限。禁止导出的策略不能通过模型租约绕过。
- 此功能不暴露为 MCP 工具，不返回 Key，不使用 stdout 交付配置内容。发现命令不等于获得执行权限。
- 用户明确选择的目标文件与备份会包含原始 Key。Monica grant 的期限、次数、撤销和锁库不适用于直连；停用需在上游撤销或轮换 Key。
- 显式指定目标；已有文件要求 `--force`，检查格式后保留无关配置，并在替换前写入私有备份。拒绝错误结构、重复 JSON 字段、符号链接、Unix 硬链接及原保险库/Monica 配置目标。
- 新文件和备份先设置 Unix 0600 / Windows 私有 ACL 再写入；使用同目录临时文件原子替换。重复的相同配置不会增加备份。
- Codex 使用 `monica_direct` provider；文件指定活动 profile 时同时更新该 profile 的模型/provider，保留权限设置。Claude 切换必要的认证/路由字段，保留其他配置。
- 原 Android payload、UUID、类型及未知字段不改写；已有的审计与根分类修复机制仍可能写库。

## 验证

全部运行测试与真实客户端联调均在 `wsl -d homoos` 中，Rust 1.97.0，Linux 原生二进制。构建输出放在 D 盘的本任务专用目录，禁用 debug 信息与增量缓存以节省空间。

回归按组完成，共 328 项通过，1 项既有 Android 回写夹具测试忽略：

- 原有库测试 257 项通过；新增直连单元测试 7 项通过，覆盖文件格式/权限、无凭据输出、合并备份、重复执行、错误输入、链接文件、Tiga 拒绝和活动 profile。
- 二进制/命令契约测试 38 项通过。
- CLI 管理端到端 20 项通过，包含无需保险库的手动 stdin 配置及从 Android 条目读取 Key 的配置。
- 剪贴板、语言、portable 集成测试合计 6 项通过。未启用 `MONICA_CLIPBOARD_TEST=1`，已有测试默认不实际读写系统剪贴板；此结果不代表验证了主机剪贴板。
- `cargo clippy --locked --all-targets -- -D warnings`、`cargo fmt --check`、`cargo build --release --locked` 通过。
- release 二进制与练习本生成数据一致：57 个发现命令、58 个教学条目。
- 文档站 12 项测试通过，VitePress 构建通过；五种语言页面经临时 HTTP 服务验证返回 200，并包含直连命令和 stdin 说明。WSL 无法访问原 npm 镜像的部分下载地址；仅在临时测试副本中换用官方 npm 下载地址，版本与 integrity 保持不变，没有修改项目锁文件。

真实客户端使用独立 HOME / CODEX_HOME / CLAUDE_CONFIG_DIR、空工作目录、禁用工具的简单请求；没有修改或终止主机正在运行的客户端。Key 通过隐藏终端输入注入，没有写入测试源码、命令参数或版本库。

| 客户端 | 上游模型 | 结果 |
| --- | --- | --- |
| Codex CLI 0.152.1 | glm-5.3-200k | 使用生成的配置直连，退出 0，收到精确的预期回复 |
| Claude Code 2.1.258 | glm-5.3-200k | 使用生成的配置直连，退出 0，收到精确的预期回复 |
| 两款客户端 | ministral-3b-latest | 上游参数错误；Claude 返回 HTTP 422，Codex 返回 invalid_request_error |

模型列表读取 HTTP 200，Key 在两款客户端 stdout/stderr 中均未出现，测试结束即删除临时含 Key 配置。模型列表可见不代表模型完整兼容 Responses / Messages 客户端参数。

验证完成后 `cargo clean` 删除本轮 6021 个构建文件（1.8 GiB）。Linux release 可执行文件单独保留在 WSL 本任务目录；没有替换主机正在使用的客户端或旧 Windows CLI 可执行文件。

## 发布边界

这是直连模式的功能和 Linux 联调验证。尚未据此发布正式 Release：Windows/macOS 的本轮二进制、真实代理链路，以及跨平台发布流水线仍需单独验证。没有修改版本号或创建标签。
