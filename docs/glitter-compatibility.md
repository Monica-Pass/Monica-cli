# Glitter 客户端兼容边界

MDBX 引擎继续支持 Glitter 格式与加密能力。Monica CLI 暂不接入该档位：引擎支持不等于客户端提供操作入口。

## 当前行为

- CLI 帮助、命令发现和档位选择仅提供 Sky / Multi / Power。旧的 `init --tiga glitter` 和 `tiga set glitter` 命令明确返回 `glitter_unavailable`，不会创建其他档位代替。
- CLI、TUI 和 broker 不创建、打开、编辑、导出或同步 Glitter。即使密码与密钥均正确，也不能绕过客户端限制。
- 已有 Glitter 数据库及配置保持不变，不自动降档、转换或删除。请保留文件，并选择明确接入 Glitter 的客户端；不要把它改名为其他档位。
- 通用 `--key-file <FILE>` 保留给 Sky / Multi / Power 的组合凭据；显式提供的无效密钥仍不能退回单密码认证。密码通过隐藏输入或受信任的 `--secrets-stdin` 输入，密钥内容不进入 argv、配置、日志或 MCP。
- 已知 Glitter 本地库在 WebDAV / 增量 / 附件网络操作前拒绝。未知远端数据库可以下载到临时区进行识别，识别为 Glitter 后不安装、不绑定同步、不下载附件、不回放增量、不上传。
- 错误恢复返回 `use_compatible_client`，不建议自动重试，也不提供在 CLI 中打开或创建 Glitter 的恢复命令。其他档位的同步、ETag 与冲突保护不变。

## 只读识别

```text
monica mdbx check <FILE> --json
```

该命令不解锁、不迁移、不修复 Android 根目录。公开文件头检测到 Glitter 时，报告：

```json
{
  "declared_tiga_profile": "glitter",
  "terminal_support": "unsupported",
  "header_authenticated": false
}
```

这是文件兼容性提示，不是加密认证或完整性证明。`declared_tiga_profile` 只输出固定枚举；任一 Glitter 档位、策略版本或关键扩展标记都会触发拒绝，不能据此授予访问权限。扩展文本在 Rust 分配前限制为 4096 字节。SQLite 可能创建 WAL/SHM 协调文件，但只读检查不修改主文件。

## 验证

测试只使用临时合成密码、密钥和本地假服务。直接调用 MDBX 原生接口创建真实加密 Glitter，验证原生解锁与加密备份仍可用；同时验证 CLI 即使获得正确材料仍拒绝，数据库字节、修改时间和已有配置保持不变。

覆盖创建、打开、管理、降档、人工密钥导出、broker、只读识别、远端识别与零网络拒绝，并回归旧档位。此前 CLI 便携接入与本地-only 测试报告仅保留为历史记录；当前结果见 [客户端退出接入验证](glitter-client-disabled-2026-10-06.md)。

源码联调需要同批 MDBX 引擎改动：本轮 CLI 的 `codex/glitter-client-boundary-20261006` 分支对应 `Monica-Pass/Mdbx` 的 `codex/glitter-engine-20261006` 分支。将两个仓库分别放在相邻的 `monica-pass-cli` 与 `mdbx` 目录；不要用尚未包含 Glitter API 的旧版引擎编译此分支。
