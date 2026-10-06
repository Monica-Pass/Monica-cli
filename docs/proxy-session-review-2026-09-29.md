# 本地模型代理会话：实现与复核

日期：2026-09-29。接续 `af50f7d`，修复五分钟新鲜认证窗口影响长期代理的问题。旧报告中的五分钟限制仍适用于未显式启动代理会话的路径。

## 使用行为

```sh
monica serve --proxy-grant my-model-client --session-minutes 60
# 同一进程使用多个现有模型授权
monica serve --proxy-grant my-model-client --proxy-grant claude-client --session-minutes 60
```

本人输入主密码后，仅选中的模型授权获得持续代理会话。现有 grant、proxy-config 和本地 Key 格式不变。启动时显示实际期限，JSON 为 ready 事件的 `data.proxy_sessions[]`，包含 `grant` 和 `expires_at_unix`。

期限取进程时长、授权剩余时间和 Tiga 原解锁会话绝对期限的最小值，Multi 默认最多两小时。代理可跨过普通五分钟新鲜认证和十分钟空闲窗口；普通秘密读取、MCP 与其他权限没有延长。未指定新参数时保留旧行为。

到期后本人重新执行 serve 并输入主密码。AI 授权尚有效且预算未用尽时，本地 Key 可以继续使用；授权到期或预算用尽才需 renew 并更新配置。重启不会恢复会话或清空调用预算。

## 修改范围

- CLI：参数与中英帮助、启动流程、每份授权的引擎会话、源条目读取、流式重检、实际期限输出、教学站数据和说明。
- MDBX：`connection/mod.rs` 的纯内存生命周期标识；`tiga_policy/credential_use.rs` 的授权和重检；`object_disclosure.rs` 的事务内授权读取入口；对应测试和说明。
- MonicaDocs：五种语言模型中转说明。

数据库格式、Android API Key payload、同步协议、现有操作枚举和默认策略均不变。会话不参与同步，旧客户端无需接入新接口。审计沿用 `reveal-secret`，老客户端能识别，但不会显示专门的代理标签。

本轮未修改 Android、F-Droid，未安装覆盖现用 CLI。MDBX 主目录已有另一任务的未提交修改，本轮仅修改上述授权相关文件。验证使用当前引擎工作区；CLI 位于 `.cli-api-key-work/monica-pass-cli`，通过同级 mdbx junction 使用引擎。整合或推送前须分别核对两个仓库的改动归属。

## 重点复核

1. 仅人类管理路径在启动时签发会话；HTTP/MCP 请求不能签发、延长或恢复它。
2. 源 UUID/版本/分类/类型、原始授权内容、设备和已解析策略均固定；变化后拒绝使用。
3. 明文读取与引擎授权重检共享事务；流中继续检查撤销、锁库、到期和来源变化。
4. 单调时钟与墙钟共同限制期限；重新解锁、更换密钥环、时钟回退均使旧会话失效。
5. 保留旧策略和审计枚举，不伪造认证时间、不改全局策略。

## 验证结果

- MDBX：`cargo +1.97.0-x86_64-pc-windows-gnu test -p mdbx-storage --no-default-features --features core,filesystem-blob-store --lib`，717 通过，包含 7 项新授权生命周期测试。
- CLI：`cargo +1.97.0-x86_64-pc-windows-gnu test --all-targets --quiet`，323 通过，1 项原有 Android 返回样本测试保持忽略。新增测试覆盖两种协议越过新鲜认证窗口、到期中断流、不回退到普通授权、调用预算跨重建保留、签发拒绝无效授权、实际期限与重启不恢复。
- 实际 CLI 子进程覆盖 bind → grant → proxy-config → renew → 带 `--proxy-grant` 的 serve → 输出实际期限 → lock → unbind，并检查输出无凭据。
- CLI fmt 与严格 Clippy（`--all-targets -- -D warnings`）通过。MDBX 本轮修改文件单独 rustfmt 检查通过。
- MDBX 严格 Clippy 未全绿：现有 attachment、conflict、entry、object_label、object_relation、object_summary、operation_coordinator、snapshot、schema/v10、sync_apply、sync_delta、sync_state 等文件共 19 个不同位置的警告，没有来自本轮修改的授权文件。普通 Clippy 退出成功；未顺带修改其他任务代码或添加 lint 豁免。引擎全局 fmt 还报告另一任务的 `mdbx-core/src/json.rs` 格式差异，本轮未改动它。
- MonicaDocs：12 项测试通过，生产构建通过。浏览器验证中英日俄越页面 HTTP 200，新参数和期限说明存在，无页面脚本错误；CLI 教学站上手页同样验证通过。
- 教学数据已按当前命令语法重新生成：54 条语法命令、55 个教学条目。

发布构建 `cargo +1.97.0-x86_64-pc-windows-gnu build --release` 通过。真实供应商、Codex 和 Claude Code 客户端的端到端连接仍未验收；本轮使用临时库与本地 TLS 模拟上游。

## GitHub 提交范围

CLI 分支为 `codex/cli-android-api-keys`，文档站为 `codex/cli-model-proxy-docs`。MDBX 分支 `codex/tiga-proxy-session` 的提交 `f81d3f1f08589670e534473ab8e0030b796dfdf4` 只包含本轮 6 个授权相关文件；通过独立 Git 索引生成，不改变共享 MDBX 工作目录的分支、索引或其他未提交修改。源码构建步骤已指定配套引擎提交，见 [克隆与构建依赖](source-checkout.md)。

上述测试记录对应推送前的本机工作区，其中还包含另一会话未提交的引擎兼容性修复；那些修复没有夹带进本轮提交。用户要求清理构建缓存后，本轮推送只调整构建版本说明，未重新构建已分离的 GitHub 提交组合。
