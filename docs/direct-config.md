# Codex / Claude Code 直连配置

`direct-config` 把原始 API Key 写入指定的客户端配置文件。客户端直接请求 HTTPS 上游，不需要运行 `monica serve`，也不需要创建 grant。支持手动输入和复用 Monica 已绑定的 Android API Key。

| | 代理 `proxy-config` | 直连 `direct-config` |
| --- | --- | --- |
| 客户端持有 | 可撤销的本地 Key | 原始上游 Key，以明文保存在私有配置文件 |
| 请求路径 | 客户端 → Monica → 上游 | 客户端 → 上游 |
| Monica 授权期限、次数、撤销、锁库 | 生效 | 不适用；锁库或删除绑定不会使已写出的 Key 失效 |
| 停用 | 撤销 Monica grant | 在上游撤销或轮换 Key，并更新客户端配置与备份 |

## 手动输入

```sh
monica direct-config manual --client codex --api-base https://models.example.test --model YOUR_MODEL --output ./codex-direct/config.toml
monica direct-config manual --client claude --api-base https://models.example.test --model YOUR_CLAUDE_MODEL --output ./claude-direct/settings.json
```

在隐藏提示中输入 Key。自动化使用 `--secrets-stdin`，由可信执行器通过 stdin 注入 `{"token":"..."}`。不要把真实 Key 放进命令参数、shell 历史、提示词或版本库。Monica 不读取环境变量中的 Key，也不在 stdout、JSON 结果或错误中输出 Key。

Codex 默认使用 Bearer；Claude Code 默认使用 `x-api-key`。第三方 Anthropic 服务需要 Bearer 时可加 `--auth bearer`。Codex 不支持 `--auth x-api-key`。

## 使用已保存的 Key

先用 `library` 找到已有条目，用 `bind` 建立本机连接，然后执行：

```sh
monica direct-config saved work-ai --client codex --model YOUR_MODEL --output ./codex-direct/config.toml
```

命令提示输入主密码；自动化 stdin 字段为 `{"password":"..."}`。它停止当前保险库的 broker，检查 Tiga `ExportData` 和 `RevealSecret` 权限，再读取绑定条目；不复用模型代理的授权租约。禁止导出的策略（如 Power）会拒绝操作。连接协议须与客户端匹配：Codex 为 OpenAI，Claude Code 为 Anthropic。Key 或端点变化后需重新绑定，不能覆盖绑定的上游地址来重定向凭据。

原生类型、UUID、payload 和未知字段保持不变。引擎授权审计以及既有的 Android 根分类修复仍可能写库。此命令是可信本机管理功能，不是 MCP 工具；命令发现不赋予执行权限。

## 地址与客户端文件

- 仅接受无用户名、密码、query、fragment 的 HTTPS 根地址。不要填写 `/responses`、`/chat/completions`、`/messages` 或 `/models` 请求端点。
- Codex：裸域名补 `/v1`；已有路径原样作为 API base。写入 `model`、`model_provider=monica_direct` 及该 provider 的 `base_url`、`wire_api=responses`、`experimental_bearer_token`，禁用该 provider 的 WebSocket。
- Claude Code：末尾 `/v1` 会移除，SDK 自己追加 `/v1/messages`。写入顶层 `model` 和 `env.ANTHROPIC_BASE_URL`，以及 `ANTHROPIC_API_KEY` 或 `ANTHROPIC_AUTH_TOKEN`。切换时移除该文件内冲突的另一种认证、OAuth、`apiKeyHelper`、`ANTHROPIC_MODEL` 和 Bedrock/Vertex/Foundry 开关；其他设置保留。
- 模型名必须显式指定，不做映射；第三方服务需要支持对应的 Responses 或 Messages 接口。

`--output` 始终必填，不自动寻找或修改主机客户端配置。文件须为 Codex 的 `.toml` 或 Claude Code 的 `.json`。新文件和备份在写入内容之前限制为当前用户可读写（Unix `0600` / Windows 私有 ACL）。

已有文件须显式加 `--force`：先验证格式，保留无关配置，修改前生成同目录 `.monica-<UUID>.bak` 私有备份，再原子替换。无变化时不生成额外备份。无法识别的结构、重复 JSON 字段、符号链接及 Unix 硬链接会被拒绝。已有 `monica_direct` provider 带有其他凭据或请求头机制时也会拒绝，需先由本人解决冲突。备份可能含旧 Key，应与配置一样妥善保管。

## 启用配置

Codex 从配置目录的 `config.toml` 读取设置；可用独立的 `CODEX_HOME` 选择本次配置目录。若文件内指定了当前 `profile`，也会同步该 profile 的模型与 provider，保留权限设置和其他 profile。Claude Code 可用 `claude --settings ./claude-direct/settings.json` 指定文件。客户端的登录、环境变量、项目配置或启动参数仍可能覆盖用户级设置，联调时应使用独立目录和干净环境。

2026-09-29：在 `wsl -d homoos` 的隔离目录中，Codex CLI 0.152.1 和 Claude Code 2.1.258 已通过第三方 HTTPS 服务的 `glm-5.3-200k` 完成真实直连并返回预期回复。该服务的 `ministral-3b-latest` 返回上游参数错误；可列出模型不代表它兼容两款客户端发出的全部请求。此结果不代表所有供应商、模型或操作系统都已验证。

这只改变新启动的客户端进程；Monica 不结束或重启正在运行的 Codex / Claude Code。切回代理时应重新使用代理配置，并处理此前写出的原始 Key。
