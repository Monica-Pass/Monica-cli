# Android 密码条目互通

CLI 支持 Android 原生 v1 `login` / `kind=password` 的本地创建、字段编辑、目录移动和删除。实现依据 Android 工程的 `docs/ANDROID_MDBX_PASSWORD_CLI_ALIGNMENT.zh-CN.md` 密码载荷契约。它与网关的 `api-token` 是独立适配器，普通密码不能被绑定为服务 Token。

## 后续代登录

2026-10-05 决定暂停 AI 代登录接入，待浏览器插件的填充接口完成、用户通知后继续。本次只保存已完成的密码存储互通和测试；没有新增自动填写、代登录或 OTP 生成/取码工具。后续需先确定目标站点校验、填充请求授权与结果回传接口，保持密码和 OTP 不进入模型。浏览器插件和桌面应用由各自项目实现。

## 本地命令

这些命令不列入 AI 命令发现，也没有对应 MCP 工具。管理操作需要主密码，先停止并锁定 broker；命令完成后需重新解锁 broker 才能继续服务调用。结果只包含原生 ID、逻辑 ID、分类、标题、类型、版本、head commit 和 Android 身份匹配状态，不含用户名、备注、密码或自定义字段。

```text
monica passwords create --id <UUID> --title <公开标题> [--category <分类ID>]
monica passwords info <原生ID>
monica passwords edit <原生ID> --expected-head <COMMIT_ID> [--title <公开标题>]
monica move <原生ID> <分类ID>
monica delete <原生ID>
```

创建时由调用者生成一次 UUID，保留它用于重试。同一 UUID、相同创建参数通过引擎持久化操作凭证去重；不同参数不能复用这个创建 ID。逻辑 ID 为 `password:<UUID>`，原生 ID 按 Android 的 vault ID + 逻辑 ID 算法生成，标题或 `password_group_id` 不参与身份计算。省略分类时使用 Android 根集合。

`info` 的 `head_commit_id` 用于下一次 `edit`。编辑时重新读取并合并完整载荷，在引擎提交事务内检查原对象的 head、类型、版本和目录。过期 head 返回 `object_changed`。一次编辑若已成功而响应丢失，重复旧 head 也会被拒绝；应重新读取并核对结果，不要自动换 head 覆盖。它提供冲突检测，不负责替用户合并并发编辑。

人可在终端隐藏输入主密码和字段 JSON。脚本使用 `--json --secrets-stdin`，由可信输入方直接送入一个严格 UTF-8 JSON 对象：

```json
{"password":"synthetic-vault-password","fields":"{\"username\":\"alice\",\"password_plain\":\"synthetic-entry-password\",\"notes\":\"第一行\\n第二行\"}"}
```

这里只展示虚构输入内容。`fields` 是一个包含 JSON 对象的**字符串**，遵循现有 secret-stdin 字段契约；整个输入上限仍是 16 KiB。`info` 只接受 `password`，不接受 `fields`。秘密不放进命令参数、环境变量或公开标题，可信输入方也不应把它们回显给 AI。单个待编辑载荷的授权读取与写回上限为 4 MiB。

编辑仅改标题时，字段输入使用 `"fields":"{}"`。查看完整秘密仍使用 [本人 TUI 查看器](mdbx-compatibility.md#本人查看完整字段)，默认掩码、60 秒清除，没有命令行明文输出或剪贴板入口。

## 字段与保留规则

`fields` 支持以下标准 snake_case 键；不支持的键会拒绝整次编辑：

| 输入类型 | 字段 |
| --- | --- |
| string | `website`、`username`、`password_plain`、`notes`、`authenticator_key`、`passkey_bindings`、`ssh_key_data` |
| string 或 null | `app_package_name`、`app_name`、`password_group_id`、`bound_note_entry_id`、`email`、`phone`、`address_line`、`city`、`state`、`zip_code`、`country`、`credit_card_number_plain`、`credit_card_holder`、`credit_card_expiry`、`credit_card_cvv_plain` |
| string、object 或 null | `wifi_metadata` |
| Android Int 范围内整数 | `sort_order` |
| 对象数组 | `custom_fields`；每项需有 string `title` / `value`、boolean `is_protected`、Int `sort_order` |

未提供的键保持原值，显式 `null` 保留为 JSON null，不等于删除键；Android 对部分 null 字段会保留本机投影，具体以契约为准。`custom_fields: []` 清空列表。嵌套 JSON 字符串、自定义字段顺序、未知字段、旧 camelCase 键、大整数、精确小数和字面 serde marker 键保持其数据语义，不保证 JSON 空白与对象键顺序不变。

Android 导入会 trim 自定义字段标题并跳过空标题，写出时按 `sort_order` 和本地 ID 排序；需要两端保持相同显示时应提供非空标题和明确排序值。

密码原文不做 trim、Unicode 归一化或本机解密。设置 `password_plain` 时写入 `monica_password_encoding=plaintext-v1`，因此以 `C2|` 或 `MDK|` 开头的真实口令仍是字面文本。未改密码时，旧 `password` 兼容字段和缺失的编码标记原样保留；CLI 不尝试恢复 Android Keystore/MDK 历史密文。

创建固定为 `login_type=PASSWORD`。已有 PASSWORD、WIFI、SSO 可编辑受支持字段；不通过这个接口转换子类型。内嵌 OTP URI、SSH/Wi-Fi raw JSON 和 Passkey 关联元数据不会自动解码重写。Passkey 关联不包含签名私钥，保留关联不代表迁移了完整 Passkey。

`kind`、逻辑 ID、原生类型、版本、文件夹 ID 和 Room 本机 ID 不能通过字段 patch 覆盖。身份不符合 Android 算法的普通 login 可返回 `android_roundtrip_identity=false`，其普通编辑、移动和删除会拒绝。未来载荷版本及未知 login 子类型保持只读。

## 目录、附件与删除

无附件的密码移动在同一引擎事务内修改原生 collection 与 `mdbx_folder_id`；移回根目录时删除 payload 中的文件夹键。目标目录缺失或已删除则拒绝；后续原生移动失败时，前面的 payload 修改也回滚。

当前依赖引擎的 `MoveEntry` 不迁移附件 collection。CLI 因此在共享写事务内检查活动附件及已删除附件引用，跨目录移动返回 `attachment_move_unsupported`，保留原位置、载荷和附件内容。此保护也适用于既有 Token/密钥的条目移动。它不提供附件迁移实现，也不绕过引擎自行改表。

删除沿用确认机制和原生 tombstone，不写 `isDeleted` payload 字段。引擎原生 restore 的载荷恢复有测试覆盖；本次没有新增 CLI 回收站或恢复命令。文件搬运继续使用已有 portable backup/blob 校验路径；这次没有新增自动云同步协议。

## 验证与边界

Rust 合成测试覆盖完整载荷保留、固定 ID 重试及重开、同组同名条目独立性、过时 head、目录回滚、附件移动拒绝、原生删除/恢复、旧密文保留和网关拒绝把 password 用作 Token。CLI 集成测试检查 secret-stdin、仅返回摘要和命令发现边界。

双向测试入口为 `scripts/check_android_passwords.py`，配套两版 Android 的 `MdbxCliPasswordAlignmentTest`。先构建并安装匹配的 debug app/test APK，然后指定测试设备和**不存在的**输出目录：

```text
python scripts/check_android_passwords.py --adb <adb路径> --serial <设备序列号> --package takagi.ru.monica --output <新目录>
```

F-Droid 使用 `--package takagi.ru.monica.fdroid`。脚本只生成合成库，保留 host 原始 manifest 作为独立预期，按 CLI 创建 → Android repository 读取/两次编辑/创建 → CLI 重开验证/编辑 → Android 重开验证运行。测试结束删除该次设备端工作文件和插入的 Room 记录，不清空应用数据。

2026-10-05 已在公共 `Monica_Issue136_API_32`（API 32，x86_64）运行普通版与 F-Droid，均通过上述四阶段。应用实际加载各自打包的 arm64-v8a 原生库，运行在该 AVD 的 ARM 兼容环境；不是原生 ARM 实体机验证。测试核对两个同名同组账号各自的密码、物理/逻辑 ID、vault ID、嵌套目录、非默认 HOTP URI、自定义字段、多应用绑定和 Passkey 关联，Android 连续写入两次后对象数仍为 2；Android 再新建一条后，两端对象数均为 3。

同日 Windows GNU 工具链 `1.97.0-x86_64-pc-windows-gnu` 的标准测试套件结果为 309 通过、0 失败、3 个外部 fixture 入口默认忽略（其中两个密码入口已由上述脚本单独运行）。`fmt --check`、`clippy --all-targets -- -D warnings` 和 `build --locked --release --jobs 1` 均通过。默认并行编译曾遇到 Windows 页面文件不足；改为 `--jobs 1`、测试 `--test-threads=2` 后完成。

这里验证的是打包 FFI 和实际 `Mdbx2Repository` 密码 writer；测试显式构造临时 Room 投影，不等于完整 UI 导入流程、实体手机或云同步验收。Android 普通 writer 当前会重建已知字段，不能保证 CLI 保留的未知顶层扩展经过 Android 编辑后仍在。SSO 关系、归档、图标、收藏和部分本机绑定也有契约列明的 Android 映射缺口；本次没有修补这些 Android 产品行为。
