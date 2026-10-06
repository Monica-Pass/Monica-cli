# MDBX 跨端兼容

新增的 [Android API Key 绑定](model-proxy.md) 只读取受支持的原生 v1 对象并保存本机引用，不扩展其编辑 Adapter，不改写原 payload 或标签。来源 head 变化会使旧授权失效。

CLI 以 [Monica 跨端存储契约](https://github.com/Monica-Pass/Monica/blob/main/docs/storage/MDBX-CROSS-CLIENT-CONTRACT.zh-CN.md) 为接入要求。MDBX3 是运行时名称，当前可写文件格式为 MDBX-2；对象的 `payload_schema_version` 独立于文件版本。文件打开、加密、提交、删除和同步均使用 MDBX 引擎。

MDBX 引擎保留 Glitter 格式与加密能力，但 Monica CLI 暂不接入。CLI、TUI 和 broker 均拒绝 Glitter 的创建、打开、管理、导出和同步，包括提供正确主密码与密钥的情况；不自动降档、转换或删除已有库。仅保留 `mdbx check` 的只读公开头识别，结果为 `terminal_support: "unsupported"`、`header_authenticated: false`。Sky / Multi / Power 与通用密钥文件能力不变。详见 [Glitter 客户端兼容边界](glitter-compatibility.md)。

## 对象与编辑

库列表保留所有原生类型、UUID 和 Collection 层级；未知对象不转换为 `login`，不进入 Gateway 或自动填充。通用详情显示原始类型、版本和所属 Collection。

CLI 可编辑的 Adapter 是原生 v1 `api-token` + `monica.gateway.credential.v1`，以及原生 v1 `login` 中受支持的 SSH/GPG 子类型和符合 Android 身份约定的 `kind=password`（PASSWORD/WIFI/SSO）。普通密码的本地创建、字段编辑、目录一致性及附件移动限制见 [Android 密码互通](android-passwords.md)。SSH 内部未来 schema 同样只读。普通移动、删除入口在写入层重新验证 Adapter；未知类型、更高版本及不理解的关键字段都会拒绝。

编辑从授权读取的原始 payload 修改指定字段。业务 JSON 使用共享核心 `mdbx_core::json`，同时保留精确数字与字面对象键（包括 serde 的特殊内部标记名称），不能直接以 `serde_json::Value` 反序列化替代。重命名不重建 payload；SSH 注释保留 `ssh_key_data` / `sshKeyData` 的选择及字符串/对象形状。未知嵌套字段、数组元数据、空字符串、`null`、布尔值和大整数保留。提交事务内核对对象的 head commit、类型、版本与 Collection，过时修改返回 `object_changed`，不会覆盖其他端刚写入的数据。

## 本人查看完整字段

运行 `monica`，在项目列表按 Enter，输入主密码打开临时只读详情。按 ←/→ 切换顶层字段，空格显示或隐藏当前值；↑/↓、PageUp/PageDown、Home/End 可滚动完整内容。非 JSON 对象显示完整原值。终端控制字符以转义文本显示，不执行其控制序列。

所有字段值默认隐藏。详情通过引擎 Tiga disclosure 授权，最多读取 4 MiB；策略拒绝、会话锁定或数据超限均明确报错。关闭、失焦或 60 秒到期后清除内容，切换字段重新隐藏。终端需支持焦点事件；固定的 60 秒期限始终有效。

该入口仅属于本人使用的 TUI。没有对应的 JSON、MCP、标准输出或剪贴板导出接口；AI 命令发现不包含通用 payload 读取命令。Gateway 只解释匹配类型、版本和 schema 的令牌，原有授权边界继续生效。

## 复制与 WebDAV

本地导入先使用引擎便携复制，再通过引擎 Blob Provider/Transfer API 保留 `<vault>.blobs`，验证引用后才切换配置。原库保留。复制验证使用独立文件，避免打开校验改变要上传的数据库字节。

远端有 `<vault>.sync`，或本地出现外置 Blob 时使用 Android 相同目录协议：

```text
<vault>.sync/streams/<device>/<generation>/segments/<sequence>-<digest>.mdbxsync
<vault>.sync/blobs/<digest前2位>/<随后2位>/<完整digest>
```

分段不可变，重复发布核对已有内容；游标包含配对的 commit/delta checkpoint、transfer ID、序号和前段摘要。目录顺序不当作因果顺序。缺父提交或精确的外键依赖错误，且引擎 checkpoint 未改变时保持待处理，只在其他流推进后重试。历史目录重新检查，完成流若又出现后续分段会明确报告。

Blob 只传密文，校验内容摘要；分段应用后若附件尚未齐全，不推进该段接收游标、不确认同步成功。下次同步先补齐附件，再幂等重放。没有 `.sync` 且没有外置 Blob 的既有远端继续使用 ETag 条件快照；目录探测失败不会降级。单文件分支的两端同时修改仍报告冲突。

429/503 共享本次客户端会话的退避期限，遵守 `Retry-After`；最多 3 次请求、最多 30 秒等待预算，服务器要求更久时退出而不提前重试。取消能中断等待、网络请求和下载。401/403、内容冲突、认证错误不盲目重试；条件覆盖结果未知时不自动重发，不可变对象可通过再次创建和读取校验恢复。

边界：单个数据库/加密 Blob 传输上限 64 MiB，一次 Blob 处理总量上限 4 GiB，最多 100,000 个本地 Blob；超限明确失败。附件本身可由多个加密块组成。没有附件垃圾回收或明文导出入口。`gateway.device.json` 保存首次远端打开时的本机同步身份。

## 验证

合成回归覆盖未知类型、未来版本、精确数值、失效编辑、策略拒绝、详情隐藏与完整滚动、本地副本、外置附件往返、缺附件后恢复、分段依赖、取消和 HTTP 退避。跨端 fixture 由 CLI 生成，Android 使用其打包的原生库独立读取，再由 CLI 回读 Android 输出。执行结果与尚未通过的环境检查见 [本次验证记录](mdbx-validation-2026-09-28.md)。要求本身不代表某个客户端的全部 Adapter 已通过验收。

2026-09-28 审查修正与复跑方式见 [无损 JSON 与跨端测试修正](mdbx-review-fixes-2026-09-28.md)。
