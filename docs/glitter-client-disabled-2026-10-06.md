# Monica CLI 暂不接入 Glitter：验证记录

## 范围

本次只撤销 Monica CLI 的客户端接入，不删除 MDBX 引擎的 Glitter 格式、加密、解锁或备份能力。不修改 Android 实现；其独立的暂不接入决定按当前项目约定保留。未使用真实密码库、账号或云服务，未发布或复制安装包。

## 客户端边界

- CLI、TUI 和 broker 共用创建/打开拒绝检查；提供正确主密码和密钥文件也不会放行。档位修改同样拒绝 Glitter，不以其他档位替代。
- 公开文件头检查使用只读连接，在可写引擎连接前拒绝 Glitter 标记。保留有界解析与原生格式复核，不将公开头当作认证。
- `mdbx check` 保留只读识别：`terminal_support: "unsupported"`、`header_authenticated: false`。帮助与机器命令发现只列 Sky / Multi / Power。
- 统一返回 `glitter_unavailable`；恢复提示不建议在 CLI 内重试、降档或打开，也不假定 Android 当前已经接入。
- 远端操作在已知 Glitter 本地库上不发网络请求；未知远端只下载到临时区识别，拒绝后不安装、不绑定、不抓附件、不上传。普通档位不得覆盖远端 Glitter。
- 原有 Sky / Multi / Power 的密码与组合凭据路径保留，错误密钥不能静默退回密码单因子。

## 整合前的旧基线验证

Windows，GNU Rust `1.97.0-x86_64-pc-windows-gnu`，合成凭据、临时库与本地假 WebDAV。

- TDD 红测：旧实现的 Glitter admission 返回 `Ok(())`，测试期望 `GlitterUnavailable`，实际失败。
- 首轮专项：10 passed、0 failed。直接由原生引擎创建真实加密 Glitter，验证原生备份与解锁仍工作、CLI 正确因素也拒绝。检查原库字节/修改时间、既有配置、未创建目标目录和导出文件，以及子进程输出不含合成密码。
- `cargo fmt --check`：通过。
- `cargo clippy --all-targets -- -D warnings`：通过。
- 命令文档生成和 `--check`：通过，51 条真实语法命令；人工密钥导出维持既有的独立教学条目。
- 普通版、F-Droid、两份候选未发布说明均保留 `## 中文` / `## English` 两区和其他条目。F-Droid 无 OneDrive 内容；当前 fastlane 24 的中文/英文长度检查分别为 499/474 字符，未修改已发布历史。

完整回归：**327 passed、0 failed、3 ignored**；最终 `cargo build --release` 成功。三个跳过项为原有独立 Android 返回样本/合成样本导出测试，本轮未新增跳过项。

| 测试集 | 通过 | 跳过 |
| --- | ---: | ---: |
| Library / TUI / broker / WebDAV | 252 | 3 |
| Binary / grammar / recovery | 39 | 0 |
| CLI management subprocesses | 19 | 0 |
| Clipboard | 2 | 0 |
| Glitter compatibility | 5 | 0 |
| Native encrypted Glitter fixture | 1 | 0 |
| Native opt-out / legacy combined credentials | 5 | 0 |
| Language | 3 | 0 |
| Portable state | 1 | 0 |

新增的旧档位组合凭据测试、共享 broker/网络拒绝测试与中英文恢复提示检查全部通过。首次全量测试暴露一条仍匹配旧提示措辞的断言，已更新为检查明确支持 Glitter 的客户端提示，并禁止推荐 Android、CLI 创建或打开；完整重跑通过。保留首次失败日志，不覆盖历史证据。

## 与远端 main 整合后的验证

在本地边界改动上合并远端 CLI `a26b0c1`，保留 1.0.101 的模型 API 代理、限时授权、direct-config 和 Windows 打包修复。配套引擎合并远端 `d1d3cc4` 后为 `f004673ef07a04c31702eaf1015e062f29730537`；CI 与源码构建说明固定这一提交。既有发布标签、发行记录和二进制不变，本次不是新版本发布。

- 合并后的绑定、解绑、direct-config 和 broker 全程传递显式组合凭据，不静默丢弃密钥文件。新增端到端红测先在 `bind` 返回 `unlock_required`，修复后完整链路通过。
- 扩展真实加密 Glitter 子进程测试：绑定、解绑、direct-config、带代理授权的 serve 均返回 `glitter_unavailable`；文件和配置保持原样，不生成客户端配置、不泄露合成密码。专项通过。
- 首次全量测试发现上游授权过期测试依赖运行耗时：快速创建时过期时间早于生效时间，配置校验先拒绝。只修正测试的历史时间窗口，另加配置合法性断言；生产安全检查未放宽。
- MDBX workspace release：1166 passed、0 failed、1 个既有手动性能基准 ignored。release 构建和 fmt 通过；普通 workspace Clippy 通过但保留旧警告，不声称严格零警告。
- CLI 严格 Clippy（all-targets、`-D warnings`）通过；production release 构建通过；重新生成的文档与实际语法一致（57 条命令、58 个教学条目）。

最终 CLI release 全量重跑：**357 passed、0 failed、3 ignored**。其中库测试 279、命令解析 39、CLI 管理子进程 22、剪贴板 2、Glitter 专项 11、语言 3、便携状态 1；3 个跳过项仍为已有独立 Android 合成样本导出／回读测试，本轮未新增跳过。该结果来自整合后的代码，不是旧基线的 327 项结果。日志位于工作区 `.codex-tasks/cli-mdbx-integrate-sync/`，首次失败日志保留。未重新进行 Android 真机、真实云账户、Linux/macOS 或 MSVC 运行验证，不将本地 GNU 结果声称为所有平台 CI 成功。

## 可复现命令

```powershell
cargo +1.97.0-x86_64-pc-windows-gnu fmt --check
cargo +1.97.0-x86_64-pc-windows-gnu clippy --all-targets -- -D warnings
cargo +1.97.0-x86_64-pc-windows-gnu test --release -- --test-threads=1
cargo +1.97.0-x86_64-pc-windows-gnu build --release
node docs/reference/build.mjs --check
git diff --check
```

专项可用 `cargo +1.97.0-x86_64-pc-windows-gnu test --release --test glitter_compatibility --test glitter_encrypted --test glitter_portable -- --test-threads=1`；共享 broker/网络拒绝覆盖位于 `sync_tests::unsupported_glitter_refuses_local_broker_and_remote_access_without_mutations`。完整日志保留在工作区 `.codex-tasks/cli-glitter-disabled/`。

此前便携接入、本地-only 报告仅为历史测试事实，不再代表当前 CLI 可用功能。本轮没有重新进行 Android 真机、真实 OneDrive 或跨平台实机测试，也未测量代码行覆盖率。
