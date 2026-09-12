# TUI 布局与源码参考

本次布局以用户提供的 Yazi 截图和 [Yazi 源码](https://github.com/sxyazi/yazi/tree/b8973fb4c2b9b184aad03cfc717c1fefe4140c40)为依据。参考版本固定为 `b8973fb4c2b9b184aad03cfc717c1fefe4140c40`，逐个阅读布局、列表、选中样式、预览、状态栏及 Rust 渲染入口。Monica 继续使用自己的 Ratatui 视图和原有管理操作，没有引入 Lua 或 Yazi 运行时。

| Yazi 源文件 | Monica 对应规则 |
| --- | --- |
| `yazi-plugin/preset/components/root.lua` | 一行路径、伸展的正文、一行状态。分类不另占标签行。 |
| `yazi-config/preset/yazi-default.toml`、`components/tab.lua` | 正文三栏比例 1:4:3。120 列为 15/60/45，80 列为 10/40/30。 |
| `components/rails.lua`、`rail.lua` | 中栏两侧各一根连续竖线，没有外围盒子、横线或重复栏标题。 |
| `components/parent.lua`、`current.lua`、`entity.lua`、`linemode.lua` | 第一条记录从正文首行开始；图标、名称、简短类型/按键；按终端字符宽度截断；选中样式覆盖整行，两端单独绘制。 |
| `components/header.lua` | 一行青色路径与筛选条件。Monica 显示 `monica://connections` 等虚拟分类路径，WebDAV 附带当前子目录。 |
| `components/status.lua`、`yazi-config/preset/theme-dark.toml` | 模式与类型在左、名称居中、状态和位置在右，使用圆角分段。Monica 显示真实代理锁定状态。 |
| `components/preview.lua`、`plugins/folder.lua`、`yazi-fm/src/mgr/preview.rs` | 预览直接使用正文高度。按信息分组、字段紧凑排列；没有占位表头或固定帮助段。 |
| `yazi-fm/src/root.rs`、`mgr/modal.rs` | 帮助、消息和输入表单按需覆盖在浏览器上，关闭后保留浏览位置。 |
| `yazi-fm/src/input/input.rs`、`help/help.rs` | 弹窗使用 Ratatui 的 `BorderType::Rounded`，四角与竖边共享矩形坐标；正文限制在边框内。 |

表中 `components/` 的完整前缀是 `yazi-plugin/preset/`。

颜色按用户截图选择：背景 `#282c34`、蓝色选中 `#61afef`、青色路径 `#56b6c2`。绿、黄、红分别用于有效只读状态、写权限或锁定提醒、失效或错误状态。授权范围仍由真实数据决定，颜色不会赋予权限。

字符宽度、换行和截断按完整 grapheme 处理，保留中文与 emoji。预览和消息的滚动上限按换行后的内容计算。推荐 Nerd Font；`:icons` 或 `MONICA_TUI_ICONS=plain` 使用普通字符，操作含义始终有文字。

视图仅消费明确公开的名称、用途、仓库、允许操作、时间和路径。绘制、筛选、帮助和图标切换不会读取保险库、Token、主密码或客户端 capability。选中项仍先按可见行映射到名称/远端路径，再交给原有管理函数。

验收图来自 Ratatui TestBackend 的真实单元格，使用合成公开数据，并非原生终端截图；PNG 字体为本机安装的 0xProto Nerd Font Mono，中文回退到微软雅黑。终端字体和行距由使用者的终端决定。

弹窗清理背景时，会同时清理跨过左右边缘的完整 grapheme。仅清理矩形内部可能留下左边半个中文字符，使终端跳过随后的边框单元格。被切掉的字符保留背景色；关闭弹窗后正常重绘原页面。

PNG 中的 `─│╭╮╰╯` 全部由同一个字符网格绘制，共享中心线、线宽与单元格接点。不能混用字体圆角和手绘竖线，否则字体基线和行高的差异会造成断口。像素回归检查覆盖四角的八处接点；TUI 回归检查覆盖全部表单、帮助和长结果在连续缩放及长中文、组合字符、emoji 输入下的边框和重绘。

[WebDAV 登录弹窗（80 × 24）](images/webdav-login.png) · [四角放大图](images/border-detail.png)

开发时可在 Windows 上用 Python、Pillow 和上述字体重新生成验收图。渲染工具仅用于开发，不是 CLI 运行依赖：

```powershell
$env:MONICA_TUI_CAPTURE_DIR = Join-Path $env:TEMP 'monica-tui-captures'
cargo +1.97.0-x86_64-pc-windows-gnu test --lib tui_
python -m unittest discover -s scripts -p test_render_tui.py
python scripts/render_tui.py $env:MONICA_TUI_CAPTURE_DIR
```

`--font-dir` 与 `--system-font-dir` 可指定终端字体、系统回退字体目录。这里只导出测试构造的合成界面，不捕获用户保险库或终端会话。

Yazi 的 MIT 许可与署名保留在 [第三方说明](../THIRD_PARTY_NOTICES.md)。
