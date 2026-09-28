# Android API Key 模型中转：主会话审查交接

用户确认的目标：复用 Monica Android 已存 API Key，生成本地地址与可撤销的本地 Key，通过 Monica 路由中转；第一版同时支持 OpenAI 和 Anthropic。

## 改动位置与整合

- CLI 分支：`codex/cli-android-api-keys`，基于 `d777af110a6886364e83083cae9d82e37f4a43b9`。
- CLI 工作目录：`C:/Users/joyins/Desktop/Monica-all/.cli-api-key-work/monica-pass-cli`。
- 文档站分支：`codex/cli-model-proxy-docs`，提交 `a7663ad`，位于 `C:/Users/joyins/Desktop/Monica-all/MonicaDocs`。
- 本次只修改 CLI 与 MonicaDocs。没有修改或推送 Android、F-Droid、MDBX 引擎，也没有使用真实凭据。
- 主 CLI checkout 仍有另一会话的 MDBX 审查修复，未覆盖、暂存或提交这些文件。整合时需合并 `src/vault.rs`、`src/object.rs`、`src/contract_tests.rs` 和 `docs/mdbx-compatibility.md` 的相关改动，保留其修复。
- 本轮构建通过 junction 使用现有 `Monica-all/mdbx`，其 HEAD 是 `5cf8af6`，并带有主会话的未提交修复；不是对干净 MDBX HEAD 的独立验证。引擎改动不包含在此 CLI 提交中。

## 实现与审查重点

| 文件 | 行为 |
| --- | --- |
| `src/api_keys.rs` | 通过授权 disclosure 读取指定 UUID，解析两种 Android 格式，固定协议、认证方式与 head；bind/unbind 不改写来源 payload |
| `src/ai_proxy.rs` | Loopback OpenAI/Anthropic 路由，本地鉴权、私有配置文件、JSON/SSE 转发与凭据反射检查 |
| `src/gateway/proxy.rs` | 范围/速率/次数/人工批准检查，上游认证注入，流中持续检查撤销、锁定、来源版本和 Tiga |
| `src/model.rs` / `src/config.rs` | `model-list` / `model-invoke` 独立权限，绑定指纹保持原有连接兼容性 |
| `src/vault.rs` / `src/sync.rs` | 仅按摘要记录候选来源，同步保留本机绑定，但来源变化时删除旧授权；旧 Token 编辑 Adapter 不接受本地 `api-key` provider |
| `src/admin.rs` / CLI 相关文件 | 管理命令、发现契约、双语帮助、next 指引；`serve --session-minutes` 默认 5、范围 1–1440 |
| `docs/reference/*` | 从真实 Clap 语法生成命令数据；新增模型中转上手步骤与三个命令说明 |

本地 Key 为现有授权 capability 的 `monica-` 表示形式。一份 Key 只选择一条绑定的连接。真实 Key 不写到 stdout、模型配置或审计日志。`proxy-config` 写的是单独的私有文件，不自动编辑其他程序的配置。

模型授权无法调用通用 MCP REST 或 Issue 工具；如人另外明确创建 `api-read`/`api-write` 服务授权，才开放已有通用 REST 通道，二者不能混合。模型请求没有自动重试、协议转换、模型映射或故障转移。

源条目支持原生 v1 `login` 的 `API_KEY`，以及原生 v1 `api-token` 的 `monica.api-token.v1` / `monica.gateway.credential.v1`。API Key 的空/null `password_plain` 不回退到旧 password；重复字段、未来格式、认证头注入和不同端点覆盖均拒绝。Android 原备注、未知字段和标签保持私密且不重写。

## 验证范围

回归覆盖：两种来源格式保留、OpenAI/Anthropic 真实 loopback HTTP、上游认证隔离、SSE 首事件及时返回、跨事件 Key 反射拦截、客户端取消、流未完成报错、锁定/撤销/过期/Tiga 失效中断、调用预算、协议隔离、重定向/反射/断线不重试、同步不恢复旧授权、CLI 绑定到续期与解绑完整流程。

最近一次全量回归：318 项通过，1 项既有 `android_returned_contract` 按原配置跳过（需要独立 Android 合成输出）。随后将上游请求统一序列化为已验证的 JSON 值，避免重复字段造成校验/转发不一致；针对代理重新执行 16 项测试全部通过，包含重复字段与大整数保真回归。fmt、Clippy `--all-targets -- -D warnings` 通过。

最终 release 构建通过；以 release 二进制校验的文档数据与真实语法一致（54 个命令）。产物位于隔离目录的 `target/release/monica-pass.exe`，未覆盖用户现有安装。

执行命令：

```text
cargo +1.97.0-x86_64-pc-windows-gnu fmt --check
cargo +1.97.0-x86_64-pc-windows-gnu clippy --all-targets -- -D warnings
cargo +1.97.0-x86_64-pc-windows-gnu test
cargo +1.97.0-x86_64-pc-windows-gnu build --release
node docs/reference/build.mjs --check
```

MonicaDocs：`npm test` 12 项通过，`npm run docs:build` 成功。五种语言的新增页面在 headless Edge 中返回 200，内容包含实际命令，未捕获脚本错误。CLI 练习站的上手页和三个新命令详情页也通过浏览器加载检查。临时预览服务已停止。

## 尚未覆盖的使用范围

这是本地原生协议中转的第一版，不等于 ccSwitch 的完整替代：未做真实供应商、Codex 或 Claude Code 客户端端到端验收，没有客户端配置自动安装、后台常驻服务、WebSocket、后台 Responses、文件/音频/图片专用接口或模型分页。

Tiga 限制保持生效。**默认 Multi 要求 5 分钟内的新鲜认证；设置更长的 serve 进程时长不会绕过它。** 达到策略上限后需要本人重新解锁，不能作为无人值守的永久中转。grant 的 TTL 与真实 Key 的保存期限独立。

凭据反射检查覆盖常见表示形式及受支持的流片段，不是任意恶意编码的完整防泄漏保证。上游收到请求后的计费或执行不能由本地撤销逆转。流失败通过断开传输报告，不伪造成功完成事件。代理审计记录授权发送元数据，不记录提示词或原始响应。

完整使用步骤见 [本地模型中转指南](model-proxy.md)。本次未推送远端或部署文档站。
