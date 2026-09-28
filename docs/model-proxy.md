# Android API Key 与本地模型中转

Monica CLI 可以为 Android 已保存的 API Key 生成本地地址和本地 Key。模型客户端向本机 Monica 发请求；Monica 校验授权，再从 MDBX 读取真实 Key，注入认证头并访问已绑定的上游。客户端只需要本地 Key。

```text
模型客户端 → 127.0.0.1:47831 + 本地 Key → Monica → HTTPS 上游 + 保险库中的真实 Key
```

一个本地 Key 对应一个连接。支持 OpenAI 和 Anthropic 原生协议，以及普通 JSON、SSE 流式响应。上游须支持所选协议；不做 OpenAI/Anthropic 互转、模型名映射或自动切换供应商。

## 1. 绑定手机上已有的条目

先通过 `monica open` 或 WebDAV 打开手机使用的 MDBX 保险库。KeePass、Bitwarden 中的条目须先在 Android 保存到 MDBX；本功能不直接连接它们。

在本人使用的终端执行：

```sh
monica library
monica bind work-ai --entry <ENTRY_ID> --protocol openai --note "编程助手使用"
```

`ENTRY_ID` 是 library 显示的原生条目 UUID。命令提示输入主密码，不需要重新输入上游 Key。`work-ai` 是本机连接名，`--note` 是明确公开给 AI 的用途。Android 原标题、备注和其他自定义字段不会自动成为公开用途。

支持以下原生 v1 对象：

| Android 存储形式 | 读取字段 |
| --- | --- |
| API Key 类型的 `login` | `kind=password`、`login_type=API_KEY`，Key 为 `password_plain`，旧格式仅在该字段缺失时读取 `password`；端点为自定义字段 `monica_api_key_url` |
| `api-token` | schema 为 `monica.api-token.v1` 或 `monica.gateway.credential.v1`，读取 `token` 与 `api_base` |

绑定保留原生类型、UUID、分类、payload、未知字段和标签，只在本机配置中记录引用。打开库时，引擎授权审计和已有的 Android 根分类修复机制仍可能写入数据库。

端点必须是 HTTPS API 根地址，例如 `https://api.openai.com/v1` 或 `https://api.anthropic.com`。单独的域名根会追加 `/v1/`；已有路径直接作为根使用。不要填写完整的 `/chat/completions` 或 `/messages` 请求地址。Android 的 website 字段不是 API 端点。

只有条目未保存端点时，才用 `--api-base https://models.example.test/v1` 补充；它不能覆盖条目中已有的不同端点。OpenAI 默认向上游发送 Bearer，Anthropic 默认发送 `x-api-key`；第三方服务确有需要时可显式指定 `--auth bearer` 或 `--auth x-api-key`。

## 2. 授权并生成本地配置

```sh
monica grant my-model-client --connection work-ai --repo "*" --operation model-list --operation model-invoke --ttl-minutes 240 --max-calls 1000
monica proxy-config my-model-client --output ./my-model-client.json
monica serve --proxy-grant my-model-client --session-minutes 60
```

`model-list` 允许查询模型，`model-invoke` 允许模型调用和计数接口。这里的 `--repo "*"` 表示该连接的模型接口范围，不开放任意 URL 或通用服务 API。这两种权限不能与 Issue、`api-read`、`api-write` 混合；模型授权也不能调用这些 MCP 工具。模型调用按写操作处理，`--approval write` 会要求本人逐次批准。

`proxy-config` 将配置写入仅当前用户可访问的文件；标准输出只显示地址、文件位置和授权到期时间。文件形状如下，示例 Key 是占位符：

```json
{
  "protocol": "openai",
  "base_url": "http://127.0.0.1:47831/v1",
  "api_key": "monica-<本地授权值>",
  "grant": "my-model-client"
}
```

让受信客户端读取文件中的 `base_url` 和 `api_key`。该文件本身是可撤销的访问凭据，应放在版本库外，不放进提示词或日志。Monica 不会自动修改 Codex、Claude Code 或其他客户端的模型配置；客户端必须支持自定义 API 地址和 Key。MCP 的 `mcp-config` 安装流程用于服务工具，模型中转使用这里的 `proxy-config`。

Anthropic 的完整配置步骤相同，只需使用另一条已保存的 Key：

```sh
monica bind work-claude --entry <ANTHROPIC_ENTRY_ID> --protocol anthropic
monica grant claude-client --connection work-claude --repo "*" --operation model-list --operation model-invoke
monica proxy-config claude-client --output ./claude-client.json
monica serve --proxy-grant claude-client --session-minutes 60
```

| 协议 | 默认本地 base_url | 本地认证 | 路由 |
| --- | --- | --- | --- |
| OpenAI | `http://127.0.0.1:47831/v1` | `Authorization: Bearer <本地 Key>` | GET `/v1/models`，POST `/v1/responses`、`/v1/chat/completions` |
| Anthropic | `http://127.0.0.1:47831` | `x-api-key: <本地 Key>` | GET `/v1/models`，POST `/v1/messages`、`/v1/messages/count_tokens` |

两个协议可共用一个 `serve` 进程，不同的本地 Key 选择各自连接。Anthropic 也接受 Bearer 本地认证；一次请求不能同时发送两种认证头。Anthropic 支持 `?beta=true`，版本头缺失时使用 `2023-06-01`。

## 3. 会话、续期和撤销

三个限制各自生效：代理进程运行时长、MDBX Tiga 代理会话与绝对期限、AI 授权的有效期和调用预算。上游 Key 的存储不会随其中任何一个到期而删除。

本人执行 `monica serve --proxy-grant my-model-client --session-minutes 60`，输入主密码，明确批准指定模型授权的一小时代理会话。需要多个授权时重复 `--proxy-grant`，例如 `--proxy-grant my-model-client --proxy-grant claude-client`。启动时显示每份代理会话的实际到期时间，JSON 的 `ready.data.proxy_sessions` 提供同样的信息。

代理会话不受普通秘密读取的五分钟新鲜认证窗口和空闲窗口影响，但不能超过原解锁会话的 Tiga 绝对期限：默认 Multi 最多两小时，从本次解锁算起；条目或分类的更严格策略仍会限制或拒绝授权。实际期限取进程时长、AI 授权剩余时间和 Tiga 绝对期限的最小值。`--session-minutes` 范围为 1–1440，默认仍为 5。

未指定 `--proxy-grant` 时保留原来的五分钟新鲜认证检查；普通 MCP、查看、复制和导出权限也不会获得延长。会话到期后需本人重新执行同一条 serve 命令，AI 授权尚未到期且预算未用完时，本地 Key 可继续使用。锁库、撤销、源条目或策略变化会停止代理；会话不能在重启或重新解锁后恢复，也没有自动解锁。

代理会话只保存在本机引擎内存中，不修改数据库格式或 Android API Key 字段，不参与同步。旧版 Android 等客户端可以继续使用现有引擎。引擎审计沿用现有 `reveal-secret` 操作值与约束，不新增旧版无法识别的枚举。

授权默认 240 分钟，最长 1440 分钟；`--requests-per-minute` 和 `--max-calls` 分别限制速率与总次数。预算持久化，重启代理不会清零。每次上游请求消耗一次，包括上游返回错误的请求。

授权到期或次数用完后：

```sh
monica renew my-model-client
monica proxy-config my-model-client --output ./my-model-client.json --force
monica serve --proxy-grant my-model-client --session-minutes 60
```

续期会轮换本地 Key，旧文件内容随即失效；客户端需要重新载入新配置。`--force` 仅用于明确替换已有输出文件。

```sh
monica revoke my-model-client
monica lock
monica unbind work-ai
```

分别撤销一个授权、锁定全部代理请求、解除本地连接绑定。`unbind` 保留 Android 原条目。更改原条目的 Key、端点或其他造成 head commit 变化的编辑后，旧绑定拒绝访问；核对手机修改，再执行原 bind 命令并加 `--replace`，然后重新 grant、生成配置。重新绑定撤销该连接全部旧授权。WebDAV 同步只保留仍匹配来源版本的授权，不会恢复已撤销的授权。

## 边界与排错

- 只监听 `127.0.0.1`，要求正确 Host，拒绝浏览器 Origin；面向本机原生客户端，不提供局域网入口或浏览器直连 CORS。
- 只请求绑定的 HTTPS 根地址，不跟随重定向、不使用环境代理、不转发本地认证头、Cookie 或任意上游认证头。Monica 不自动重试模型请求；客户端自己的重试策略需自行配置。
- 请求上限 16 MiB，上传限时 30 秒；响应上限 64 MiB，单个 SSE 事件上限 1 MiB；最多 4 个并发上游模型请求，单次请求限时 300 秒。
- 检查常见明文/编码形式的凭据反射和受支持的跨事件文本片段；这不是针对任意恶意编码的完整数据防泄漏系统。授权撤销、锁定、Tiga 失效或客户端断开会停止继续转发，已到达上游的请求仍可能计费或完成。
- 未提供 WebSocket、后台 Responses、文件/音频/图片专用端点、模型列表分页、协议转换、自动故障转移或系统后台服务。模型名及能力由上游决定。
- `401`：本地 Key 失效、授权到期或预算用完。`403`：协议/权限不匹配、策略拒绝或保险库锁定。`409`：源条目变化，需重新绑定。`429`：速率限制。流中途失败会中断传输，不生成成功完成事件。

已用临时 MDBX 和本地 TLS 模拟上游验证接口、SSE、权限与密钥隔离；不代表已完成真实供应商、Codex 或 Claude Code 客户端端到端验收。
