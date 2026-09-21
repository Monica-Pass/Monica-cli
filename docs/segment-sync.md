# CLI 分段流同步设计（与 Monica Android 互操作）

状态：设计稿，未实施。写这份文档的目的是让 `webdav sync` 从「整文件替换」升级为引擎的分段合并协议，
从而能真正与手机上的 Monica 共用同一个远端保险库。当前实现（`src/sync.rs`）对这类远端库一律拒绝，
返回 `remote_protocol_unsupported`，见第 1 节。

引用位置相对于本仓库根目录。`../mdbx` 与 `../Monica-main` 由另一个会话维护，本文只读引用，不要求它们改动。
`AND = ../Monica-main/Monica for Android/app/src/main/java/takagi/ru/monica`。

## 1. 为什么单文件模式永远做不到互操作

实测（2026-09-21，`dav.jianguoyun.com`，账号 lichaoran8@gmail.com，`webdav list Mdbx`）：

```
目录 0        Mdbx/Monicacli.mdbx.sync
文件 532480   Mdbx/Monicacli.mdbx        <- 一次性初始副本，一直没变
本地 770048   D:\Apps\MonicaCLI\data\Monicacli.mdbx
绑定          etag=null, remote_sha256 == local_sha256 == a3b2956f…28a16, last_sync=1789223390
```

`.sync` 下的层级（每层都是独立的 Depth:1 PROPFIND 实测结果）：

```
Monicacli.mdbx.sync/streams/monica-android-628f86f9-451a-498a-a44e-cdcfb920396e/<26 个 UUIDv4 代目录>/segments/0000000000-<64hex>.mdbxsync
```

26 个代目录，每个只含 1 个分段，4 984 – 31 716 字节，没有 `blobs/`（该库无外置附件）。

两条致命结论：

1. **`.mdbx` 是不可变的初始副本**，Android 只在首次发布时写它（`CREATE_ONLY`，
   `AND/repository/Mdbx2RemoteSyncCoordinator.kt:78-118`）。之后所有变更都写进 `.sync`。
   所以 CLI 拿它做哈希比较，永远得到「远端没变」，本地有改动就报 `Uploaded`/`UpToDate` —— 这正是
   手机上看不到 CLI 数据、CLI 却以为同步成功的原因。
2. **CLI 覆盖它才是真的破坏**：每台设备的游标都以该文件的哈希/ETag 为基准，覆盖后其他设备会判定分叉。
   所以「放宽 ETag 判断」这个方向是错的，不能做。

因此 `synchronize()` 在任何读写之前先探测 `<path>.sync`（`src/sync.rs` 的 `segment_sync_managed`），
探测失败（列目录报错）视为「未检测到」，普通单文件对端行为不变；`src/sync_tests.rs` 用假服务器钉住了
「报 `remote_protocol_unsupported` 且一个 PUT 都不发」。

## 2. 线上格式（必须逐字节一致，不是「大致相同」）

远端路径拼接规则在 `AND/utils/MdbxRemoteTransport.kt:54-88`：

| 资源 | 路径 |
| --- | --- |
| 同步根 | `<remote vault path>.sync` |
| 分段流根 | `<sync>/streams/<deviceId>/<generationId>/segments/` |
| 分段文件 | `%010d-<64 位小写十六进制>.mdbxsync`，序号取自 `manifest.segment_index`，**每 (设备, 代) 从 0 重新计数** |
| 附件 | `<sync>/blobs/<id[0..2]>/<id[2..4]>/<id>`，`id` 是整个文件字节 SHA-256 的小写十六进制 |

分段文件名里的摘要 **不是文件哈希**，而是 bincode 载荷摘要：
`incremental_bundle_payload_sha256`（`../mdbx/crates/mdbx-sync/src/bundle.rs:627`）。
CLI 自己校验时别把两者混用，否则永远对不上。

文件字节是**带认证的封装**（Android 写的是 `write_incremental_bundle_authenticated`，v8，
`bundle.rs:565`；magic `MDBXSYNC` + u32 版本 + 20 字节保留区（前 8 字节是载荷长度）+ bincode 载荷 +
SHA-256(载荷) + HMAC-SHA256 标签）：

- HMAC 密钥 = `conn.keyring().integrity_subkey`，必须 32 字节（`bundle.rs:870`，
  `../mdbx/crates/mdbx-ffi/src/sync_facade.rs:1604-1612`）。
- 读侧 `read_bundle_file_with_limits_authenticated`（`bundle.rs:1206`）对 v7–v10 **没有密钥就拒绝解析**。
  CLI 只在解锁状态下同步，`src/vault.rs:173` 已经在检查 `keyring().is_some()`，可以直接取到同一密钥。
- **未决风险（实施第一步就要验）**：同一库在不同设备上的 `integrity_subkey` 是否恒定。若密钥纪元轮换后
  每台设备各自派生，跨客户端 HMAC 校验会直接失败，那时需要的是引擎侧协商，而不是 CLI 绕开校验。

载荷类型都带 `serde(deny_unknown_fields)`，格式串 `INCREMENTAL_BUNDLE_FORMAT = "mdbx-sync-incremental-v1"`：
`IncrementalSyncBundle { manifest, commits, auxiliary_deltas }`，
`IncrementalBundleManifest { format, vault_id, source_device_id, exported_at, transfer_id, segment_index,
previous_segment_sha256, is_last, base, result, commit_inventory, delta_inventory }`
（`bundle.rs:124-149`）。`IncrementalBundleCheckpoint { commit_inventory: Option<String>, delta_inventory: Option<String> }`
两两皆 `None` 是显式的分段起点标记（`bundle.rs:101-109`，`peer_sync.rs:105-111`）。令牌本身是 JSON 字符串，
但**绑定 vault**，只能当不透明值存取（`../mdbx/crates/mdbx-storage/src/repo/commit_inventory.rs:331-349`）。

## 3. 引擎侧 API：CLI 直接调用，不需要改 `../mdbx`

`PeerSyncService`（`../mdbx/crates/mdbx-storage/src/peer_sync.rs`）：

```text
current_checkpoint(&conn)                                        -> IncrementalBundleCheckpoint   :81
export_incremental_segment(&conn, device_id, &base, resume, opts)-> IncrementalSyncBundle         :91
apply_incremental_segment(&mut conn, device_id, &bundle, &base, resume) -> ApplyBatchResult       :243
next_resume(&bundle)                                             -> Option<IncrementalBundleResume> :307
```

- `apply` 依次校验：已解锁 → 设备 ID → base 成对 → resume 形状 → `bundle.validate()` → `vault_id` 相等
  → `manifest.base == expected_base` → 分段链（`:250-305`）。**重复应用不是错误**，返回
  `applied_commits=0, skipped_commits>0`，所以「下载后崩溃再重试」是安全的。
- 错误全是 `StorageError`，没有细分变体：base 不匹配 / 链断裂 / 载荷损坏都被压成
  `ConstraintViolation` 或 `Validation("sync protocol error: …")`（`:262-268`、`:608-621`）。
  CLI 侧必须自己区分「需要重新拉流」和「传输损坏」，映射到不同 `GatewayError`，不能把损坏报成分叉。
- 前置条件：表 `vault_meta`、`commits`、`commit_operations`、`commit_parents`、`tombstones`、
  `commit_inventory`、`sync_delta_batches` 必须存在。`validate_device_id` 只要求去空白后非空且 ≤256 字节。

## 4. CLI 要新增的本地状态

分段协议的核心是「每台设备只写自己的流」，所以 CLI 需要一个稳定设备 ID 和一个游标文件，
`gateway.json` 里的 `RemoteBinding` 装不下这些（它是整文件语义）。

```text
deviceId   gateway.json 增加 webdav_device_id：monica-cli-<UUIDv4>，生成后永不复用旧值
游标文件   gateway.sync.json（0600，与配置同目录）
  { vault_id, bootstrap_sha256,
    export:  { generation_id, base_checkpoint, resume: {transfer_id,next_segment_index,prev_sha256}|null,
               next_sequence, pending_local_sha256 },
    streams: { "<deviceId>/<generationId>":
                 { next_sequence, checkpoint, resume, last_applied_digest, blocked_reason } } }
```

对应 Android 的 Room 表 `mdbx_sync_states` 与 sidecar `sync-state-<databaseId>.json`
（`AND/data/MdbxSyncState.kt:23-32`、`AND/utils/MdbxSyncSidecarStore.kt:14-68`）。语义照搬，字段自己命名即可，
**不需要与 Android 交换状态文件**。游标丢失可以靠全量重放恢复，但会退化成第 6 节的一次昂贵首连。

## 5. 拉取算法

1. 列 `<sync>/streams` → 设备目录；逐个列设备目录 → 代目录；只处理游标里 **未见过的** 代目录。
   已应用的代目录靠游标跳过，所以稳态开销是 2 次 PROPFIND + 新分段下载；首次接入需要
   `1 + 设备数 + 代目录数`（本机实测 26 代）次列表，属一次性成本。
2. 对每个待处理代目录列 `segments/`，解析 `<序号>-<摘要>`（序号补零 10 位、摘要 64 位小写十六进制），
   过滤掉自己的 `deviceId`（Android 同样这么做，`Mdbx2RemoteSyncCoordinator.kt:420`），按 (设备, 代) 分组、
   序号升序，**只应用 `sequence == next_sequence` 的分段**；出现空洞就记录 `missing segment N` 并停在该流，
   本轮最多推进 4 趟（`:429`、`:888`）。
3. 每个候选分段下载后：校验载荷摘要与文件名一致 → 用 `apply_incremental_segment(&mut conn, 来源设备,
   bundle, 该流 checkpoint, 该流 resume)` 应用 → 用 `next_resume` 推进游标。任何一步失败都不动游标。
4. 全部流都推进到 `is_last` 且本地无待推分段，即视为已追平。

合并发生在引擎里（提交级），CLI **绝不**自己做三方哈希判断或整文件替换。

## 6. 推送算法

1. `current_checkpoint` 取导出基准；若上一轮 `is_last` 已置位（或游标为空），以 `resume = None` 开新代，
   引擎会 mint 新的 `transfer_id` 作为 `<generationId>`（`peer_sync.rs:210-217`）。
2. 循环 `export_incremental_segment(...)` → `incremental_bundle_to_bytes`（认证版）→ 计算载荷摘要 →
   `PUT <stream>/segments/%010d-<摘要>.mdbxsync`，**只用 `If-None-Match: *`**（不可变、只创建）。
3. 写完必须 GET 回来核对载荷摘要，与第 5 节一致再推进游标。坚果云不支持强 ETag（实测 4 个远端条目
   `etag` 全为 `null`），因此**不能**依赖 `If-Match`；同理，若服务器忽略 `If-None-Match`，误覆盖只能靠
   写后核对发现，所以这一步不是可选项。同名已存在时按 Android 的做法：字节相同 → 当作成功跳过，
   不同 → 报冲突（`AND/webdav/WebDavConditionalWriter.kt:27-44`、`WebDavMdbxFileSource.kt:206-228`）。
4. 附件库（`blobs/`）留到后续阶段：先确认 `mdbx-storage` 的 `filesystem-blob-store` 特性在
   `default-features = false` 下的实际行为，再决定是否纳入第一版。第一版继续对带外置附件的库报
   `external_blobs_unsupported`。

## 7. 与现有单文件模式共存

- 绑定里加 `mode: "file" | "segment"`；`segment` 由「检测到 `.sync`」或用户显式指定触发。
  两条路径共用 profile 与锁，但游标文件互不覆盖。
- `remote_protocol_unsupported` 保留，直到分段模式真的可用；届时它对普通网盘误判的兜底仍然有用。
- `open_remote` 目前只读初始副本，接入分段模式后必须改成「下载 bootstrap + 重放分段」，
  否则用户打开的仍然是第 1 节那个过时快照。
- 自动同步仍不引入后台常驻：手动 `webdav sync` 与 TUI 的 `S` 触发，与现在的授权纪律一致。

## 8. 传输层缺口（`src/webdav.rs`）

1. **没有建目录能力**。需要 `MKCOL`（Android 走 Sardine 的 `createDirectory`，失败后吞掉错误再用
   PROPFIND 确认存在，`WebDavMdbxFileSource.kt:270-289`）。CLI 要同样把「已存在」当成成功。
2. `list()` 固定 `Depth: 1` —— 这正好符合逐层遍历的需求，**不要**改成递归 PROPFIND：
   递归在多数网盘上要么被降级要么巨慢，且 Android 也是三层遍历。
3. `normalize_path` 允许 `.`、`-`、字母数字，UUID 与 `.mdbxsync` 都能原样拼接；补一条测试钉住
   `<64hex>` 与 `%010d` 拼出的路径不被规范化拒绝。
4. 每次分段读写都要过 `reject_secret_value`；分段字节是密文，列表里的路径含设备 ID，注意别把
   用户名/设备 ID 误判成秘密，也别让错误消息回显摘要之外的元数据。

## 9. 依赖与工具链

- `PeerSyncService` 已经可达：`peer_sync` 在 `../mdbx/crates/mdbx-storage/src/lib.rs:25` 是无条件模块，
  而 CLI 已依赖 `mdbx-storage`（`Cargo.toml:25`，`features=["core"]`）。
- 要显式命名 bundle 类型需要加 `mdbx-sync = { path = "../mdbx/crates/mdbx-sync", default-features = false }`。
  `mdbx-sync 0.2.0` 已在 `Cargo.lock:1527-1540` 中解析，其依赖 `bincode`、`hmac` 也已在锁文件里，
  **不引入任何新 crate**；`zstd-compression` 特性必须保持关闭（锁文件里没有 `zstd`）。
  改完 `Cargo.toml` 必须验证 `cargo build --offline` 仍通过（GNU 工具链）。

## 10. 分期与验收

阶段 0（已完成，提交 `56cd8c8`）：检测到 `.sync` 就拒绝，真实坚果云端点已实测——
`{"code":"remote_protocol_unsupported"}`，本地配置与保险库 SHA-256 前后一致，远端大小仍 532480。

阶段 1：认证封装的读/写单测。用真引擎在临时目录造两个设备，导出 → 落盘 → 重新读回 → 应用，
断言文件名摘要 = 载荷摘要、错误分类正确、重复应用走 `skipped_commits`。此阶段完全不联网。

阶段 2：假服务器端到端。扩展现有 `FakeWebDav` 支持 MKCOL 与「忽略 `If-None-Match`」「写后返回不同字节」
两种故障，验证第 6 节第 3 条的写后核对确实拦得住。

阶段 3：CLI↔CLI 真实互通（一次性临时网盘目录），再做 CLI→Android 单向：手机先建库，CLI 重放；
然后 CLI 推分段，手机拉取。验收判据是手机上能看到 CLI 新增条目，且 CLI 崩溃/断网后重跑不产生重复条目。

阶段 4：文档与 `webdav status` 暴露模式与游标；README 的「不支持增量同步目录」改为按模式说明。

每阶段的硬门槛都是 `cargo fmt --check`、`clippy -D warnings`、`cargo test --offline --no-fail-fast` 全绿，
并在真实二进制上跑一遍对应流程，不接受只读代码得出的结论。

## 11. 未决问题（不要在实现时才想到）

1. `integrity_subkey` 是否跨设备恒定（第 2 节）——不通则整个方案依赖引擎侧改动，须先与 Android 会话确认。
2. 远端 `.sync` 树只增不减：26 代已是 26 个目录，网盘容量与列表延迟会随提交数增长。引擎与 Android 都没有
   压缩/裁剪（compaction）路径，CLI 单方面裁剪会破坏其他设备游标。第一版接受增长，但要写进 SECURITY.md。
3. 坚果云对 `If-None-Match` 的真实语义未测（当前只能确认 PUT 能创建）。若它无条件覆盖，写后核对是唯一防线。
4. 首连的重放成本：26 段 × (GET + apply)。需要进度输出与可中断恢复，不能让人对着黑屏等待。
