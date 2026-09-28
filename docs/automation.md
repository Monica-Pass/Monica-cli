# CLI 自动化

**简体中文** · [English](automation.en.md)

Monica 的 TUI 和 CLI 共用管理实现。人类可使用隐藏输入和短命令；AI 或脚本可提交公开参数，由可信本地执行器提供凭据，并读取结构化结果。MCP 继续只暴露已授权的服务操作。

## 发现命令

```sh
monica-pass commands --summary --json
monica-pass commands add --json
monica-pass commands webdav open --json
```

命令目录直接来自实际参数解析器，包含完整命令、缩写、参数、默认值、可选值、互斥组和所需凭据字段。命令路径也接受缩写。机器目录使用固定英文说明；去掉 `--json` 可查看当前界面语言的帮助。

## 先发现，再核对执行契约

摘要只包含路径、别名、用途、目标、信任边界、所需秘密字段与是否需要子命令。要执行某条命令前，再读取 `monica commands grant --json`，不要把摘要当作完整参数表。默认 `commands --json` 仍返回完整语法树。

| 完整目录字段 | 用途 |
| --- | --- |
| `path` / `aliases` | 推荐命令名与兼容写法 |
| `execution_command` | 执行 JSON 的稳定 command 标识 |
| `arguments[].role` / `value_names` | 区分 CONNECTION、GRANT、数据库及分类 ID |
| `secret_input.required` | 受信执行器须从 stdin 注入的字段；与执行逻辑共用 |
| `semantics.target` / `prerequisites` | 操作对象和执行前提 |
| `semantics.effects[]` | `{when, effect}`；如 `--install` 才合并写客户端配置 |
| `semantics.trust_boundary` | 本地查询、受信管理或授权代理通道 |
| `semantics.retry` | 重做前需要核对什么；renew 每次都会轮换 capability |
| `semantics.discovery_grants_authority` | 恒为 false：发现不授予权限 |

正式名称为 `connections`、`mcp-config`、`renew`；旧 `list`、`settings`、`refresh` 和缩写继续接受。执行 JSON 的 `command` 保持旧值，分别为 `list`、`settings`、`refresh`；`execution_command` 明确给出这个映射。

`mcp-config GRANT` 即使不带 `--install` 也会保存 Monica 的配置片段。`library`、`keys`、`tiga show` 需要解锁并会先停代理；不能仅凭“查看”二字把它们当作纯查询。

`next --grant GRANT --json` 的 `steps[].done` 只反映观察到的状态，客户端状态为 `unverified`。先配置客户端，再运行前台 serve；check 在另一终端运行。命令模板中的占位符需替换，所有步骤保留相同 `--config`。

## 常用选项

| 选项 | 作用 |
| --- | --- |
| `--json` / `-j` | 输出 JSON，并禁止交互提示。 |
| `--non-interactive` / `--no-prompt` | 禁止交互提示，保留普通输出格式。 |
| `--secrets-stdin` | 从标准输入读取凭据 JSON；必须由可信程序直接注入。 |
| `--config FILE` / `-C FILE` | 指定本次使用的管理配置。 |
| `--lang en` / `-l en` | 设置本次人工提示语言；JSON 字段和错误码不变。 |

全局选项可放在子命令前后。没有子命令时，普通模式进入 TUI，JSON / 非交互模式查询状态。`tui` 不接受自动化模式；`mcp` 的标准输入输出专用于 MCP，不能与 `--json` 或 `--secrets-stdin` 混用。

## 操作对应表

| 管理操作 | CLI 示例 | 凭据输入字段 |
| --- | --- | --- |
| 建库、保存连接、创建只读授权 | `add work --repo org/repo --note "项目用途"` | `password`, `token` |
| 单独建库 | `init` | `password` |
| 仅保存连接 | `connect work --provider github --note "项目用途"` | `password`, `token` |
| 列表 / 详情 | `connections` / `show work` | 无 |
| 修改用途 | `note work "新的公开用途"` | `password` |
| 通用服务 API 请求 | `call GRANT --request request.json` | 无；需已解锁代理和明确 API 授权 |
| 更换 Token（撤销旧授权） | `token work` | `password`, `token` |
| 列出数据库 / 切换数据库 | `databases` / `use ID` | 无 / `password` |
| 浏览分类与条目 | `library` | `password` |
| 新建 / 重命名分类 | `category TITLE --parent ID` / `rename-category ID TITLE` | `password` |
| 移动条目或分类 | `move ID TARGET_CATEGORY_ID` | `password` |
| 创建授权 | `grant reader --connection work --repo org/repo --ttl-minutes 60 --max-calls 200` | `password` |
| 续期授权（换发新 capability） | `renew reader` / `renew reader --ttl-minutes 60 --max-calls 50` | `password` |
| 给授权设人工门槛 | `grant reader … --approval write` / `renew reader --approval all` | `password` |
| 撤销授权 | `revoke reader` | 无 |
| 获取并保存 MCP 配置 | `mcp-config reader` | 无 |
| 顺带写进 AI 客户端自己的配置文件 | `mcp-config reader --install claude`（`cursor` / `codex` / `vscode`） | 无；改动前先备份，合并失败即原样退出 |
| 验证 MCP 工具发现 | `check reader` 或 `check --client FILE` | 无；代理需要已解锁 |
| 打开本地 MDBX 副本 | `open vault.mdbx` | `password` |
| 解锁并运行代理 | `serve` | `password` |
| 锁定并等待代理结束 | `lock` | 无 |
| 查看状态 | `status` | 无 |
| 读取网关审计（自己做过哪些调用） | `audit --json` / `audit --grant READER --limit 200 --json` | 无 |
| 登录 WebDAV | `webdav login --url https://dav.example.com/monica/ -n user` | `webdav_password` |
| 浏览 WebDAV | `webdav list [folder]` | `webdav_password` |
| 打开远端 MDBX | `webdav open vault.mdbx` | `password`, `webdav_password` |
| 发布 / 同步 MDBX | `webdav publish vault.mdbx` / `webdav sync` | `password`, `webdav_password` |
| 查看 WebDAV 配置 | `webdav status` | 无 |
| 保存语言偏好 | `language zh-CN` | 无 |

表中的名称必须精确匹配。`show` 选择连接；`mcp-config`、`check`、`revoke`、`renew` 选择授权。不存在的名称不会回退到其他条目。快速添加为连接和授权使用同一个名称，默认只读、有效期 240 分钟（`--ttl` 可指定 1–1440 分钟，`--max-calls` 可限制上游调用次数，省略为不限次数）；`-w` / `--allow-write` 才会增加创建 Issue 权限。详细授权用 `--op create-issue` 指定写操作。任何授权都会到期，不存在长期有效的选项。

`grant` 与 `renew` 都需要先停止代理，执行完代理保持锁定；续期后要重新 `serve` 解锁，并让 AI 客户端重启对应的 MCP 入口，才会读到换发后的新 capability。

`--approval`（`off|write|all`）与无人值守不搭：门槛要求有人当场回答，而 `--secrets-stdin` 启动的代理常常没有可应答的终端（`--json`、管道、后台都是）。这种代理是**失败关闭**的——需要批准的调用等满 15 秒后返回 `approval_timeout`，请求不会发出，也不消耗总调用次数（每分钟限额照扣）；如果启动时已有授权带门槛，代理会先打印一行"没有终端可问"的提示。流水线里请把授权留作 `off`，用 `--max-calls` 与较短的 `-t` 收窄风险；要门槛就得把代理跑在人自己的终端里。快速添加 `add` 没有该参数，它签出的授权一律是 `off`。

## 凭据输入协议

完整 GitLab / GitHub API 能力通过 `api-read` / `api-write` 加 `--repo "*"` 明确授权；请求文件、MCP 格式和重试约定见[通用服务 API](service-api.md)。旧的 Issue 授权保持原范围。

`--secrets-stdin` 接受一个 **UTF-8 JSON 对象**，总大小最多 **16 KiB**，生产者写入后必须关闭管道。仅接受该命令需要的字段，字段值必须为非空字符串；拒绝未知字段、重复字段、额外字段、`null`、非 JSON 和超限输入。密码中的空格与 Unicode 原样保留。

- `password`：保险库主密码。不设最小长度，但不能留空或仅含空白字符；打开远端库时是远端库的主密码。
- `token`：要保存的服务 Token。
- `webdav_password`：WebDAV 密码或应用密码，仅在这次进程中使用。

人工建库需要输入两次主密码。程序化建库使用生产者提供的单个密码值，不需要重复字段。输入缓冲区及解析后的凭据由 `Zeroizing` 管理；命令不会生成明文凭据文件。

AI 可以构造下面这条仅含公开参数的命令：

```sh
monica-pass add work --repo org/repo --note "项目 Issue 跟踪" --json --secrets-stdin
```

可信执行器负责获取凭据并连接到该子进程的 stdin。不要让模型生成含真实凭据的 JSON，不要把凭据放在 `echo`、Shell here-string、进程参数或环境变量中。没有配置可信输入通道时，命令会明确失败，不会尝试让 AI 获取原文。

下面是可在本地人工终端运行的 Python 启动器示例。它隐藏询问凭据，只把 Monica 的结果交给调用方；接入现有凭据管理器时，在可信执行器中替换 `getpass` 两行即可：

```python
import getpass
import json
import subprocess
import sys

credentials = {
    "password": getpass.getpass("Vault password: "),
    "token": getpass.getpass("Service token: "),
}
result = subprocess.run(
    ["monica-pass", "add", "work", "--repo", "org/repo",
     "--note", "Project issue tracking", "--json", "--secrets-stdin"],
    input=json.dumps(credentials, ensure_ascii=False).encode("utf-8"),
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    check=False,
)
sys.stdout.buffer.write(result.stdout)
sys.stderr.buffer.write(result.stderr)
raise SystemExit(result.returncode)
```

这个启动器属于可信侧，不把 `credentials` 或输入 JSON 返回给模型。正常 AI 服务访问只需 MCP capability；它不能代替管理操作所需的主密码。与其他同用户进程一样，拥有任意文件读取、环境检查或调试权限的 AI 工具必须通过系统账户或沙箱进一步隔离，详见 [安全边界](../SECURITY.md)。

## 结果与退出状态

有限时长的 JSON 命令在 stdout 输出一个 JSON 对象，成功和错误均不混入人工提示。例如：

```json
{"ok":true,"command":"note","data":{"name":"work","note":"项目 Issue 跟踪"}}
```

失败返回 `ok: false` 和 `error.code`、`error.message`。缺少凭据还会返回 `error.required`，例如 `["password", "token"]`。退出码：`0` 成功，`1` 执行失败，`2` 参数错误。拒绝的参数值与凭据内容不会出现在错误里。

`serve` 是长驻命令：启动后立即输出 `event: "ready"`，停止后输出 `event: "stopped"`，每行一个 JSON 对象并立即刷新。`add --serve` 先输出添加结果，再输出代理事件。添加成功后即使代理启动失败，已创建的连接与授权仍然存在，应根据结果继续处理。

需要访问保险库的管理或同步会先请求锁定代理，等待在途操作结束；无法取得锁时明确失败。这些操作完成后代理保持锁定，`add -s` 可在添加后直接解锁运行。查询元数据和撤销授权无需停止代理。普通解锁会话为五分钟，不延长授权有效期。授权窗口或调用次数用尽时代理返回 `reauthorization_required`，只有人工 `renew 授权名` 续期才能恢复；`status` 会显示每个授权的 `calls_used`、`max_calls`、`expired`、`refresh_required` 与 `approval` 档位。被门槛挡下的调用返回 `approval_denied` 或 `approval_timeout`：请求没有发出、不写去重日志也不记 `authorized` 审计行，因此不会重复上游写入；但 `approval_denied` 后必须停下等待本人新指示，`approval_timeout` 后也须得到本人同意，才可用同一 `request_id` 和完全相同的参数重试。重试不会绕过提问。

通过 `--secrets-stdin` 注入的 WebDAV 密码不会跨 CLI 进程保存，也不落本机凭据管理器：每次网络操作仍需重新注入，所以单条 CLI 命令结束即完成会话退出。人在终端隐藏的输入会在请求成功后记入本机凭据管理器，之后的网络操作不再索要该密码，`webdav forget-password` 删除它。TUI 的 `:logout` 只结束其自己的 WebDAV 会话。地址与用户名可以保存，`webdav status` 可查询这些公开信息与是否已存密码。

`status` 与 `webdav status` 的 `safe_remote_replace` 表示当前连接是否有强 ETag：未连接为 `null`，缺失为 `false`。为 `false` 时可以读取、检查同步状态及发布到新文件名，但本地有改动时不能覆盖原远端文件；请使用 `webdav publish NEW_NAME.mdbx`。TUI 的总览和 WebDAV 预览也会提示这一限制。


CLI 错误新增 `error.recovery`：固定的 `retry` 策略、建议命令模板、`automatic_retry: false` 与 `use_same_config`。它不回显被拒参数，也不表示可以自动执行建议。缺少安全输入应回到受信执行器；需要确认不能自动加 --force；授权到期请人 renew。write_outcome_unknown / request_id_conflict 先核对上游和审计，不能换新 request_id 盲目写入。解析错误也提供发现入口；MCP stdio 不混入这些 CLI 恢复字段。

## MDBX 跨端边界

未知类型和未来 payload 版本保留在原生摘要中，普通编辑、移动或删除返回 `object_read_only`。受支持 Adapter 仅修改已知字段，过时提交返回 `object_changed`。AI 不得通过 TUI 通用详情提取秘密；JSON/MCP 不提供通用 payload。

`.blobs` 密文随托管副本和 WebDAV 分段保留。分段与引用附件都确认后才推进游标；`blob_unavailable`、`sync_cancelled` 需检查状态并恢复后重试，不能清空游标或强制覆盖。完整约束及验证见 [MDBX 跨端兼容](mdbx-compatibility.md)。
