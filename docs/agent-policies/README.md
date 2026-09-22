# 接入 AI 客户端：两条命令

这个目录里是可以直接投递给 AI 客户端的规则文件；配好 MCP 入口用的是
`monica settings <grant 名> --install <客户端>`。

## 最小接入

```console
monica grant work-agent --connection work --repo example/project --approval write
monica settings work-agent --install claude
```

第一条签发授权并打印一次性的 MCP 配置入口文件；第二条把同一条 MCP 条目写进
Claude Code 自己的配置文件。之后重启该客户端的 MCP 入口即可。
不加 `--install` 时 `settings` 只打印配置和落一份 `clients/<授权名>.client.mcp.json`，
适合项目级配置或本工具没写进过的那种客户端。

## 各客户端写到哪里

| `--install` 取值 | 被写的文件（用户级） | 条目所在的键 |
| --- | --- | --- |
| `claude` | `~/.claude.json` | `mcpServers` |
| `cursor` | `~/.cursor/mcp.json` | `mcpServers` |
| `codex` | `~/.codex/config.toml` | `[mcp_servers.<授权名>]` |
| `vscode` | `~/.vscode/mcp.json` | `servers` |

Claude Desktop 不在表里：它的应用数据目录随平台不同，本机没有验证过，
所以仍然走"打印 JSON 自己粘"的路子。项目级配置文件（`.mcp.json`、
`.cursor/mcp.json`、`.vscode/mcp.json`）同理，用打印出来的那段 JSON 手工粘贴。

## `--install` 会做什么、不会做什么

- 只合并，不覆盖：文件里其他键、其他 server 条目原样保留。
- 会改动就先备份：备份文件名形如 `config.toml.monica-<时间戳>-<序号>`，路径会打印出来。
- 读不回来的文件不碰：不是合法 JSON、顶层结构不对、Codex 用了 `[[mcp_servers]]`
  数组写法、同一个授权名定义了两遍——这些都直接报错退出，配置文件保持原样，
  错误码是 `client_config_unusable`。
- 重复执行是幂等的：条目已经是这一份时就报"未作改动"，不再产生新备份。
- 写进去的只有可执行文件路径和 `mcp --client <文件>`：Token、主密码、capability
  都不会进入客户端配置。
- 两个副作用：JSON 客户端文件会被按 JSON 规范化重写（键序变成字母序）；
  合并后的文件权限收紧为仅所有者可读写（客户端配置里可能有别家的 `env`）。
  想还原直接拷回备份文件即可。

## 规则文件放在哪里

本目录的 [AGENTS.md](AGENTS.md) 是给 AI 看的那一份，内容自足。各家读取"仓库规则"
的文件名不同，把同一个文件拷过去改名即可（下面这些位置按各家公开约定列出，
本机只验证了上面表里的 MCP 配置路径，规则文件位置请在你自己的客户端里确认一次）：

- Claude Code：项目根 `CLAUDE.md`，或用户级 `~/.claude/CLAUDE.md`。
- Codex CLI：项目根 `AGENTS.md`（文件名正好一致）。
- Cursor：`.cursor/rules/monica.mdc`，需要在文件头加上该格式要求的
  `description` / `alwaysApply` 头部。
- VS Code Copilot：`.github/copilot-instructions.md`。

规则正文与 [`docs/ai-guide.md`](../ai-guide.md) 的 A 节同源：A 节还额外带了一张
工具参数表，那份表跟着代码变，所以本文件不带它，参数以
`monica_list_connections` 的回包为准。两处都改时请记得同步，目前**没有**
自动校验保证它们一致。
