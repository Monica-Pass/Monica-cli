# MDBX 兼容对齐验证（2026-09-28）

> 本文保留首次对齐的历史结果。后续原生库、补丁、默认测试入口及回读预期已修正；当前结果和复跑方法见 [审查修正记录](mdbx-review-fixes-2026-09-28.md)。下方旧运行时哈希不代表当前库。

本次范围是 Monica CLI 的对象适配、本人通用查看、原生提交与附件同步，以及验证过程中发现的 Android FFI 数值精度问题。依据 [跨端兼容契约](https://github.com/Monica-Pass/Monica/blob/main/docs/storage/MDBX-CROSS-CLIENT-CONTRACT.zh-CN.md)。以下结果不代表所有 Android 历史业务 Adapter 都已无损验收。

## 已验证行为

| 契约要求 | 实现与证据 |
| --- | --- |
| 未知类型、未来原生版本保持身份 | 列表保留 UUID、type、version、Collection；普通编辑、移动、删除在写入层拒绝 |
| 已知类型保留未来字段 | API Token 和 SSH 注释基于原 JSON 修改；保留未知嵌套字段、缺失/null、数组、SSH 字符串/对象形状及字段别名 |
| 过时编辑不能覆盖新提交 | 原生事务内检查完整 ObjectSummary；失效返回 `object_changed` |
| 授权后才读取明文 | 本人 TUI 详情调用受控 disclosure；4 MiB 上限；默认逐字段隐藏；关闭、失焦、锁定和 60 秒到期清理 |
| 机器输出不新增通用秘密读取 | JSON/MCP/剪贴板没有通用 payload 入口；Gateway 仅接受已知 type/version/schema |
| 原生快照与外置附件 | 引擎便携复制与 Blob Provider/Transfer API；独立副本验证；原始 fixture 文件字节保持不变 |
| 分段与 Blob 配对确认 | 分段及引用 Blob 齐全后推进接收游标；缺附件后恢复，幂等重放 |
| 因果关系与失败处理 | 只对精确缺父提交/外键错误且 checkpoint 不变的段等待依赖；其他流推进后重试；损坏游标明确失败 |
| 远端目录与覆盖边界 | `.sync` 或本地外置 Blob 选择分段；目录探测失败不降级；快照覆盖需要强 ETag 条件写 |
| 退避与取消 | 429/503 共享退避、最多 3 次请求和 30 秒等待预算；网络和下载可取消；不盲重试结果未知的条件覆盖 |

## 执行结果

| 检查 | 结果 |
| --- | --- |
| `cargo fmt --check` | 通过 |
| `cargo clippy --all-targets -- -D warnings` | 通过 |
| `cargo test` | 301 通过，0 失败；1 个跨端回读测试默认 ignored，另行执行 |
| CLI 测试组成 | library 239、binary 38、management 18、clipboard 2、language 3、portable 1 |
| `cargo build --release` | 通过，Rust 1.97.0 GNU / Windows |
| 教学站生成与 `build.mjs --check` | 通过；51 条机器发现命令，52 个教学条目；本人密钥导出教学独立于机器发现 |
| Android 原生库静态 ABI | x86_64、arm64-v8a、armeabi-v7a 均具备 539/539 必需符号 |
| Android 主版引擎设备冒烟 | 3 通过，0 失败 |
| 主版原生双向 fixture | 1 设备测试通过；处理原始 fixture 和便携副本两条路径，CLI 独立回读通过 |
| F-Droid 源码构建与双向 fixture | Gradle 源码构建/测试 harness 安装通过；1 设备测试通过，CLI 独立回读通过；使用原有 Cargo 增量缓存，三 ABI 裁剪后字节与主版一致 |
| 主版完整工程 + MDBX/WebDAV JVM | 测试包构建通过；52 个测试类、210 通过，0 失败/错误/跳过 |
| F-Droid 完整工程 + MDBX/WebDAV JVM | 测试包构建通过；52 个测试类、210 通过，0 失败/错误/跳过 |
| 主版实际应用设备回归 | 5 通过，0 失败；包含未知对象 UI、Repository 保护、同步依赖、CLI 往返 |
| F-Droid 实际应用设备回归 | 5 通过，0 失败；同一组场景 |
| 实际应用输出 → CLI 独立回读 | 主版与 F-Droid 均通过，各回读两份便携输出 |
| 真实云、真实账号、实体设备、armeabi-v7a 运行 | 未执行；arm64 在公共 x86_64 模拟器兼容环境运行 |

设备为已有公共 `Monica_Issue136_API_32`，Android API 32，x86_64，`emulator-5554`。本任务复用它，没有新建 AVD。主版/F-Droid 的独立 harness 使用各自引擎模块和原有 Kotlin 绑定，限定 x86_64；随后实际主版与 F-Droid APK 均以 arm64-v8a 在该模拟器兼容环境运行。绑定加载时验证 239 个 API checksum。实际未知对象页面截图经检查，默认字段隐藏、显示切换和返回入口正常。

测试 fixture 含未知 recovery-kit、未来版本 API Token/login、可编辑 Token、嵌套分类、Unicode、空值、30 位整数、高精度小数和分块外置附件。Android 编辑并移动 CLI 创建的 Token，创建自己的 Token 和删除标记，然后输出两份便携副本。CLI 再验证稳定 ID、类型、版本、完整字段、分类、删除状态及附件明文（仅合成内容，内存断言）。

### 发现并修复的原生精度问题

修复前真实 APK 原生库把 `123456789012345678901234567890` 读取为 `1.2345678901234568e+29`，高精度小数也被舍入；设备测试因此失败。没有放宽断言。修复是在原生工作区启用 `serde_json/arbitrary_precision`，随后原测试及双向编辑回读通过。

主版三 ABI 库已重建。F-Droid 保持源码构建，仅在 vendored `Mdbx-ffi/Cargo.toml` 启用同一特性，不添加预编译库。两端都有 `90005c8-json-precision.patch` 及重建说明。

### 运行时来源

- CLI 引擎：工作区 `mdbx`，基准提交 `5cf8af697727095654884e0a76bec6ca12d0ce6b`；CLI 依赖统一启用高精度 JSON 和 filesystem Blob Provider。
- Android：上游 `90005c8c608c952093a4522ffa507a562e2e39a4`，原 read-session / sync-delta-limits 补丁，加本次 JSON precision 补丁。
- 构建：Rust 1.86.0、cargo-ndk 4.1.2、NDK 28.2.13676358、API 21、`mdbx3-release`；保留 ELF build ID。
- 重建源码与 F-Droid vendored 源码经 LF 归一化一致。主版旧 provenance 的部分工作树哈希无法由归档字节复现，新 provenance 记录 LF 归一化哈希并保留旧值；不声称复现了旧二进制的字节。

| ABI | 主版 SHA-256 |
| --- | --- |
| x86_64 | `ed1434e7d5399d0cea59f57d441ef60ca28c4b16e9410b2b3f769687494bebc8` |
| arm64-v8a | `b13091d1dec341bde6366e074ce68f5d419cf97f5d839d57afb33a7ee6657134` |
| armeabi-v7a | `e7d4e03fd4c325f87af6df4926f362f3210d69fe5221272ff2d1ae7276c4540b` |

### 完整应用检查

首次主版应用构建在已有 `AppLauncherIconSettings.kt` 引用的五个缺失资源上失败：`launcher_icon_selection_description`、`launcher_icon_default`、`launcher_icon_blue_star`、`launcher_icon_language_hint`、`launcher_icon_refresh_hint`。随后工作区资源已补齐，本次未修改这些无关界面文件。最终两套工程重新运行 `:app:assembleDebugAndroidTest :app:testDebugUnitTest --tests takagi.ru.monica.*Mdbx* --tests takagi.ru.monica.webdav.*`，均通过，各 210 项 JVM 回归。随后主版和 F-Droid 的 debug APK 及测试包安装成功，各运行 5 项实际应用设备测试，均通过，UI 截图已检查。两端实际 APK 生成的便携输出再次由 CLI 独立回读通过。

## 当前复跑入口

普通 Android 设备测试使用已提交的合成 fixture，无需手动准备私有目录；工作目录每次独立并自动清理。完整动态往返使用 CLI `scripts/check_android_contract.py`；当前命令、输入来源和验证边界见 [审查修正记录](mdbx-review-fixes-2026-09-28.md)。
