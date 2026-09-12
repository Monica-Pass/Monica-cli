# 界面语言

Monica CLI 内置 `en` 和 `zh-CN`。语言只影响人工界面：CLI 帮助、密码提示、TUI、操作结果和错误说明。连接名称与公开备注使用用户输入的原文；MCP 的工具名、字段、错误码和 JSON 结构与语言无关。

## 选择与保存

- `--lang en`、`--lang zh-CN` 或 `--lang auto` 指定本次运行的语言。
- `monica-pass language en` 保存偏好；`monica-pass language` 查看当前设置。
- TUI 的 `F2` 切换语言并保存，也支持 `:lang en`、`:lang zh-CN` 和 `:lang auto`。
- 优先级为命令参数、`MONICA_LANG`、已保存偏好、系统语言。自动模式识别 `LC_ALL`、`LC_MESSAGES`、`LANG`；Windows 还可读取系统区域语言。不支持的系统语言使用 English。

偏好使用配置文件旁的 `*.preferences.json`，通过现有原子写入和私有文件权限保存；不会触发建库或授权更新。MCP stdio 入口不读取人工语言偏好文件。

CLI 的 `--json` 模式使用固定的机器结果与错误格式，`commands --json` 使用英文参数目录；自动化不依赖已保存的界面偏好。普通 `commands`、`--help` 和隐藏提示仍按所选语言显示。运行参数定义位于 `src/cli.rs`，新增参数时也需补充帮助翻译。

## 维护翻译

语言解析和偏好存储位于 [i18n.rs](../src/i18n.rs)，内嵌文案位于 [messages.rs](../src/i18n/messages.rs)，CLI 帮助适配位于 [cli_language.rs](../src/cli_language.rs)。

每条文案都有编译期检查的 `Message` 标识，以及放在一起的英文、中文翻译。新增文案时在目录中添加一条，再通过 `tr!(language, MessageName)` 使用。带参数的文案使用命名占位符，例如 `tr!(language, LanguageSaved, language = language.name())`；翻译中必须保留相同的占位符名称。

增加语言时，扩展 `Language`、`LanguageChoice`、区域识别与目录宏，并为全部消息添加对应翻译。不要把翻译后的字符串用于权限判断、路由或选中记录的身份标识。

语言按值传入界面，后台操作返回结构化结果，在显示时使用当前语言。表单切换时移动现有输入缓冲区，秘密字段仍由 `Zeroizing` 管理；不要将秘密作为翻译参数。用户数据只插入一次，不作为嵌套模板解析。

## 验证

测试覆盖语言优先级、偏好持久化、CLI 帮助、固定 JSON 输出、模板占位符、错误码、切换时的选择和输入保留，以及两种语言下所有表单的边框与连续缩放。使用临时配置和合成数据；渲染图来自 Ratatui TestBackend。

```sh
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --all --check
```

当前 Windows 工作区使用 `cargo +1.97.0-x86_64-pc-windows-gnu`。
