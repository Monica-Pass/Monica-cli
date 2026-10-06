# 无损 JSON 与 Android / CLI 契约测试修正

2026-09-28，后续修正记录。此前独立审查确认：旧 Android 原生库舍入精确数字；开启 `arbitrary_precision` 后，又会把合法的特殊对象键误读为内部数字标记。本次保留精度修复并解决该回归。

## 数据边界

共享核心新增 `mdbx_core::json::from_str/from_slice`。先按真实 JSON 结构读取原始成员，再构造对象、数组和精确数字，字面对象键不会进入 serde 的 Number/RawValue 标记分支。保留完整 JSON 值和数字精度，不承诺排版或键顺序。拒绝畸形 JSON、无效 UTF-8、尾随内容和过深嵌套。

覆盖原生 FFI 读写、事务写入命令、冲突合并读取、KeePass 导出读取、同步扩展以及 CLI Token/SSH 编辑。同步状态扩展通过原始字段解析，避免 serde flatten 缓冲阶段提前损坏数字；重复的同步状态字段会被拒绝。没有修改数据库 schema 或 UniFFI API。CLI 客户端配置合并也改用相同解析器；在临时配置上先复现并验证了同类字段丢失修复，未改写真实客户端配置。

以下内容现在同时保留，不把对象替换为数字或 null：

```json
{
  "counter": 123456789012345678901234567890,
  "fraction": 1.2345678901234567890123456789,
  "extension": {"$serde_json::private::Number": "123"},
  "other": {"$serde_json::private::RawValue": "null"}
}
```

Android 继续基于 `90005c8c608c952093a4522ffa507a562e2e39a4` 加四个明确 overlay；新增 `90005c8-json-literal-keys.patch`。F-Droid 保持源码构建；主版替换三 ABI 库。共享核心的同样修复基于较新的 `5cf8af6`，未把整个新核心直接升级进 Android。来源记录保留历史验证及最终文件哈希。

## 测试入口

普通 `MdbxCliContractInstrumentedTest` 使用已提交的合成 fixture ZIP，无需手动往设备私有目录放文件。每次使用唯一的临时目录，并在结束时清理；不遗留会阻止下一次测试的固定输出文件。错误密码必须返回引擎明确的 `validation error: incorrect credential`，任意其他异常不算认证负例通过。

动态跨端验证使用显式参数 `monicaContractInputDir` 和 `monicaContractOutputDir`，只接受测试应用私有目录；输入为 CLI 合成库，显式输出必须是新目录。驱动脚本负责二进制安全传输、独立输出、取回、CLI 回读和设备临时文件清理。不会清空应用数据。

示例（先构建并安装相应应用与测试 APK）：

```powershell
$env:MONICA_CONTRACT_FIXTURE_DIR = '<new-host-fixture-directory>'
cargo +1.97.0-x86_64-pc-windows-gnu test --lib synthetic_cross_client_fixture_and_portable_copy --locked
python scripts/check_android_contract.py --fixture '<host-fixture-directory>' --output '<new-host-output-directory>' --package takagi.ru.monica --serial emulator-5554 --toolchain 1.97.0-x86_64-pc-windows-gnu
```

F-Droid 的包名使用 `takagi.ru.monica.fdroid`，输出目录另取新名字。其他平台可省略 `--toolchain` 使用满足项目要求的默认工具链。`--adb` 可以指定 adb 的完整路径。运行前复用公共 AVD，并确认没有其他 instrumentation 正在执行。

Android 新对象和删除标记的 ID 由原始 fixture 预先确定。CLI 根据原始输入独立推导期望的编辑、分类、标题、类型及版本，Android 不再提供“期望 payload”清单。验证成功后，CLI 在可丢弃副本中分别删除未知字段、改错版本、移到错误分类，确认同一验证器逐项拒绝；原始返回库字节保持不变。

## 验证范围

本轮共享核心、存储、FFI、同步四模块的 Rust 库测试共 965 项通过。三 ABI 各通过 539 个必需导出符号检查。独立原生 harness 的五项测试通过，默认契约测试再次运行通过，动态双向往返与两份输出的损坏负例通过。

普通版和 F-Droid 均完成 debug APK、测试 APK 及选定 MDBX/WebDAV JVM 测试，各 210 项通过。实际应用使用公共 API 32 x86_64 模拟器的 ARM64 转译运行；独立 harness 使用 x86_64。CLI 完整常规测试 301 通过；最后的配置合并修正另跑 15 项相关回归通过，fmt、Clippy 与最终 release 构建通过。普通版及 F-Droid 各 5 项实际应用设备回归、默认契约重复运行、动态回读和损坏负例通过。最终执行汇总保存在本轮审查证据及两端 provenance 中。

边界：没有实体 ARM、armeabi-v7a 执行或真实云账号互通验证；也不宣称所有历史 Android Adapter 都完成无损改造。构建复用 Cargo 缓存，相同裁剪后的字节一致不能代替两套独立 clean build 的可复现证明。
