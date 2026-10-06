# CLI Glitter 便携接入验证

> 历史记录：本文的 CLI 接入结论已被 2026-10-06 的客户端暂不接入决定取代。MDBX 引擎保留 Glitter，CLI/TUI/broker 现仅只读识别并拒绝操作，不能按下述实验步骤使用。当前验证见 [客户端退出接入验证](glitter-client-disabled-2026-10-06.md)。下文数据保留，不代表当前功能。

范围：主密码＋密钥文件，不依赖指纹或可信硬件。继续使用原生 Glitter 加密格式、组合包装与 512 MiB / 10 次 / 4 lanes Argon2id。未发布或复制交付文件。

## 先前便携实验验证（历史记录）

以下数据对应早先允许同步的实验实现，不代表最终产品允许 Glitter 同步。最新决定为完全本地使用；本地-only 复验记录见后续章节。

GNU Rust 1.97，优化测试：325 passed、0 failed、3 ignored。

| 测试集 | 通过 | 跳过 |
| --- | ---: | ---: |
| library / TUI / broker / WebDAV | 252 | 3 |
| binary / grammar / secret-input | 38 | 0 |
| CLI management subprocesses | 19 | 0 |
| Clipboard | 2 | 0 |
| Glitter compatibility | 5 | 0 |
| Native encrypted Glitter fixture | 1 | 0 |
| Portable Glitter and CLI workflows | 4 | 0 |
| Language | 3 | 0 |
| Portable state | 1 | 0 |

三个跳过项是已有的独立 Android 返回样本/样本导出测试，不是隐藏的失败；本报告不声称完成真机跨客户端回传或真实 OneDrive 测试。

具体覆盖：

- 普通 Standard 上下文创建真实加密 Glitter，核验实际 KDF 参数；无伪造 TrustedHardware。
- 主密码和原始密钥文件同时正确才可打开；缺少密钥、错误密钥、错误密码、短于 32 字节的创建密钥均失败。短文件在创建任何数据库前拒绝。
- 真实 CLI 子进程通过 --secrets-stdin＋--key-file 完成 init、密码条目创建、library 和 tiga show，检查 stdout/stderr/config 不含秘密或密钥路径。
- TUI 的密码表单传递显式启动密钥文件，不改变旧表单字段或保存路径；缺失文件不会退回密码模式。
- broker 启停、重复锁定成功，锁后读取仍被拒绝。测试发现并修复了旧服务关闭路径重复锁定误报 StateUnavailable 的问题。
- 历史实验曾验证两个独立配置/数据库通过本地 HTTPS 假 WebDAV 完成发布、下载打开、修改、上传和另一端下载；这一路径已按最新产品决定禁用，不是当前可用功能。
- Glitter 明文密钥导出在解析条目名称之前拒绝，目标文件不创建；原有其他档位行为不变。
- 旧模式、授权边界、MCP、加密载荷保真、ETag/冲突和增量同步回归通过。

## 命令与证据

执行命令：

```text
cargo +1.97.0-x86_64-pc-windows-gnu fmt -p monica-pass-cli
cargo +1.97.0-x86_64-pc-windows-gnu clippy -p monica-pass-cli --all-targets --no-deps -- -D warnings
cargo +1.97.0-x86_64-pc-windows-gnu test --release --no-fail-fast -- --test-threads=4
```

原始成功及失败日志保留在工作区 .codex-tasks/glitter-cli-onedrive/tasks/cli-portable/，核心成功证据为 final-clippy.log 和 final-full-release-tests.log。最终 `cargo build --release` 通过（2 分 59 秒，final-release-build.log）；`cargo fmt -p monica-pass-cli --check` 通过，生成教学文档检查确认 data.js 与实际 51 条命令语法一致。

## 使用边界

Glitter 创建使用 `monica init --tiga glitter --key-file <FILE>`。后续需要打开数据库的命令沿用同一参数。TUI 使用 `monica --key-file <FILE> tui`；TUI 新建表单仍明确创建 Multi，Glitter 可先从 CLI 创建再打开。

密钥文件至少 32 字节、最多 1 MiB。密码保持现有非空约束，不新增指纹、硬件断言或密码长度门槛。原地把旧库更改标签为 Glitter 仍不支持；应创建新的 Glitter 库。详细说明见 [Glitter 本地使用](glitter-compatibility.md)。

## 最新本地-only 行为与复验

Glitter 保留本地创建、打开、编辑、broker、加密备份和手动文件搬运。WebDAV 远端打开、发布、同步及底层 Blob / 增量路径返回 `glitter_local_only`。已有本地 Glitter 在联网之前拒绝；未知远端仅允许临时加密文件下载以识别档位，识别后不安装、不绑定、不回放、不上传。旧同步配置和云端文件不删除，其他档位同步不变。

测试先记录了预期失败：原实现仍能发布 Glitter，而测试要求拒绝（local-only-red.log）。首次全量复验发现损坏远端文件的错误码发生变化，已恢复原有 `InvalidVault` 约定，保留旧测试期望。

最终优化全量复验：**326 passed、0 failed、3 existing ignored**。library 252、binary 39、CLI management 19、clipboard 2、compatibility 5、encrypted fixture 1、portable Glitter 4、language 3、portable state 1。三个跳过项仍为原有独立 Android 样本相关测试；不据此声称真机或真实云账号验证。

- 已有 Glitter 的发布、同步、远端打开：零 HTTP 请求，配置字节不变，远端不出现新文件。
- 旧存量同步绑定，以及直接 Blob / segment / bootstrap 调用：拒绝，不生成同步设备配置、不请求网络。
- 未知远端 Glitter：每次仅一条 GET，无本地库安装和配置绑定，云文件原样保留；有密钥、未提供密钥两种情况均验证。
- 旧档位本地库遇远端被换成 Glitter：只读探测后拒绝，不覆盖云文件，不更改本地配置。
- 加密备份手动本地打开及编辑、双因素正确性、明文导出拒绝、broker 重复锁定和全部旧档位同步回归通过。

证据：`local-only-final-full-release-tests.log`、`local-only-final-clippy.log`、`local-only-final-release-build.log`；GNU Rust 1.97，clippy 全 target、warnings denied 通过。最终 release 构建通过（2 分 01 秒）；`cargo fmt -p monica-pass-cli --check`、`git diff --check` 通过，教学文档检查确认与实际 51 条命令语法一致。未提交、发布或复制交付文件。
