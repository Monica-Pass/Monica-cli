# CLI 自动化

**简体中文** · [English](automation.en.md)

Monica 的 TUI 和 CLI 共用管理实现。人类可使用隐藏输入和短命令；AI 或脚本可提交公开参数，由可信本地执行器提供凭据，并读取结构化结果。MCP 继续只暴露已授权的服务操作。

## 发现命令

```sh
monica-pass cmds --json
monica-pass cmds add --json
monica-pass cmds dav open --json
```

命令目录直接来自实际参数解析器，包含完整命令、缩写、参数、默认值、可选值、互斥组和所需凭据字段。命令路径也接受缩写。机器目录使用固定英文说明；去掉 `--json` 可查看当前界面语言的帮助。

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
| 建库、保存连接、创建只读授权 | `a work -r org/repo -n "项目用途"` | `password`, `token` |
| 单独建库 | `n` | `password` |
| 仅保存连接 | `c work -p github -n "项目用途"` | `password`, `token` |
| 列表 / 详情 | `ls` / `show work` | 无 |
| 修改用途 | `e work "新的公开用途"` | `password` |
| 通用服务 API 请求 | `call GRANT --request request.json` | 无；需已解锁代理和明确 API 授权 |
| 更换 Token（撤销旧授权） | `token work` | `password`, `token` |
| 列出数据库 / 切换数据库 | `db` / `use ID` | 无 / `password` |
| 浏览分类与条目 | `tree` | `password` |
| 新建 / 重命名分类 | `mkdir TITLE --parent ID` / `rename-category ID TITLE` | `password` |
| 移动条目或分类 | `mv ID TARGET_CATEGORY_ID` | `password` |
| 创建授权 | `g reader -c work -r org/repo -t 60 --max-calls 200` | `password` |
| 续期授权（换发新 capability） | `rf reader` / `rf reader -t 60 --max-calls 50` | `password` |
| 撤销授权 | `rv reader` | 无 |
| 获取并保存 MCP 配置 | `m reader` | 无 |
| 验证 MCP 工具发现 | `ck reader` 或 `check --client FILE` | 无；代理需要已解锁 |
| 打开本地 MDBX 副本 | `o vault.mdbx` | `password` |
| 解锁并运行代理 | `u` | `password` |
| 锁定并等待代理结束 | `lk` | 无 |
| 查看状态 | `st` | 无 |
| 登录 WebDAV | `dav in -u https://dav.example.com/monica/ -n user` | `webdav_password` |
| 浏览 WebDAV | `dav ls [folder]` | `webdav_password` |
| 打开远端 MDBX | `dav o vault.mdbx` | `password`, `webdav_password` |
| 发布 / 同步 MDBX | `dav p vault.mdbx` / `dav s` | `password`, `webdav_password` |
| 查看 WebDAV 配置 | `dav st` | 无 |
| 保存语言偏好 | `lang zh-CN` | 无 |

表中的名称必须精确匹配。`show` 选择连接；`m`、`ck`、`rv`、`rf` 选择授权。不存在的名称不会回退到其他条目。快速添加为连接和授权使用同一个名称，默认只读、有效期 240 分钟（`--ttl` 可指定 1–1440 分钟，`--max-calls` 可限制上游调用次数，省略为不限次数）；`-w` / `--allow-write` 才会增加创建 Issue 权限。详细授权用 `--op create-issue` 指定写操作。任何授权都会到期，不存在长期有效的选项。

`grant` 与 `refresh` 都需要先停止代理，执行完代理保持锁定；续期后要重新 `u` 解锁，并让 AI 客户端重启对应的 MCP 入口，才会读到换发后的新 capability。

## 凭据输入协议

完整 GitLab / GitHub API 能力通过 `api-read` / `api-write` 加 `--repo "*"` 明确授权；请求文件、MCP 格式和重试约定见[通用服务 API](service-api.md)。旧的 Issue 授权保持原范围。

`--secrets-stdin` 接受一个 **UTF-8 JSON 对象**，总大小最多 **16 KiB**，生产者写入后必须关闭管道。仅接受该命令需要的字段，字段值必须为非空字符串；拒绝未知字段、重复字段、额外字段、`null`、非 JSON 和超限输入。密码中的空格与 Unicode 原样保留。

- `password`：保险库主密码。不设最小长度，但不能留空或仅含空白字符；打开远端库时是远端库的主密码。
- `token`：要保存的服务 Token。
- `webdav_password`：WebDAV 密码或应用密码，仅在这次进程中使用。

人工建库需要输入两次主密码。程序化建库使用生产者提供的单个密码值，不需要重复字段。输入缓冲区及解析后的凭据由 `Zeroizing` 管理；命令不会生成明文凭据文件。

AI 可以构造下面这条仅含公开参数的命令：

```sh
monica-pass a work -r org/repo -n "项目 Issue 跟踪" --json --secrets-stdin
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
    ["monica-pass", "a", "work", "-r", "org/repo",
     "-n", "Project issue tracking", "--json", "--secrets-stdin"],
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

`serve` / `u` 是长驻命令：启动后立即输出 `event: "ready"`，停止后输出 `event: "stopped"`，每行一个 JSON 对象并立即刷新。`add --serve` 先输出添加结果，再输出代理事件。添加成功后即使代理启动失败，已创建的连接与授权仍然存在，应根据结果继续处理。

需要访问保险库的管理或同步会先请求锁定代理，等待在途操作结束；无法取得锁时明确失败。这些操作完成后代理保持锁定，`a -s` 可在添加后直接解锁运行。查询元数据和撤销授权无需停止代理。普通解锁会话为五分钟，不延长授权有效期。授权窗口或调用次数用尽时代理返回 `reauthorization_required`，只有人工 `rf 授权名` 续期才能恢复；`st` 会显示每个授权的 `calls_used`、`max_calls`、`expired` 与 `refresh_required`。

通过 `--secrets-stdin` 注入的 WebDAV 密码不会跨 CLI 进程保存，也不落本机凭据管理器：每次网络操作仍需重新注入，所以单条 CLI 命令结束即完成会话退出。人在终端隐藏的输入会在请求成功后记入本机凭据管理器，之后的网络操作不再索要该密码，`dav forget-password` 删除它。TUI 的 `:logout` 只结束其自己的 WebDAV 会话。地址与用户名可以保存，`dav st` 可查询这些公开信息与是否已存密码。

`status` 与 `dav st` 的 `safe_remote_replace` 表示当前连接是否有强 ETag：未连接为 `null`，缺失为 `false`。为 `false` 时可以读取、检查同步状态及发布到新文件名，但本地有改动时不能覆盖原远端文件；请使用 `dav p NEW_NAME.mdbx`。TUI 的总览和 WebDAV 预览也会提示这一限制。
