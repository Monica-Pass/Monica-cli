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

主页沿用同一套外壳，只替换三栏的内容：左栏是数据库列表，当前库用 `●` 标记，图标区分已解锁、待输入密码与非当前；中栏是一棵递归树，分类作为它下面那些行的表头出现（不再是 `+ `、`· ` 这类同级标记），找不到归属的条目补在根层，`/` 搜索整库而不只是当前分类；右栏显示选中行的真实详情（掩码 Token、分类路径、绑定的服务连接与授权）。未解锁时中栏列出可执行的操作行，而不是提示文字。状态栏的按键提示按 ` · ` 分组，宽度不足时整组丢弃，绝不显示半个按键。

`/` 的匹配是子序列打分而不是子串包含：`sk` 能找到 `ssh-key`，`db` 能找到 `database url`，中文标题按字符直接匹配，空格仍然逐词 AND。分值只有四项——命中字符 16、整词落在词首 16（每词最多一次）、与上一命中字符相邻 12、首末命中之间每个未命中字符 −2。对齐取最左，另外只在更靠后的词首起点上重算一次取更高分；全动态规划的对齐不值每次按键的开销。主页与连接、授权、WebDAV 三个管理页都按分数从高到低排：Rust 的排序稳定，所以同分的行保持页面本身的顺序，主页再按"分类路径 + 名称"兜底。选中项在重排后按行 ID 复原，光标不会因为一次输入跳到别的记录上。

`y` / `Y` 把选中密钥条目的公开字段写进系统剪贴板：`y` 是 SSH 公钥行或 GPG 用户 ID，`Y` 是指纹。可选字段集中在一处 `public_field`，它只读已经解出的公开摘要——私钥材料不在那个结构里，Token 也没有任何到剪贴板的路径，命令行同样不写剪贴板。Windows 直接复用已有的 windows-sys 调 `OpenClipboard` / `SetClipboardData`，不新增依赖，别的窗口占着剪贴板时重试 8 次再提示；macOS 交给 `pbcopy`，Linux 依次试 `wl-copy`、`xclip -selection clipboard`、`xsel --clipboard --input`，先启动成功并完成写入的那个胜出，文本只走子进程 stdin、绝不拼进参数，一个都没有才提示本机没有可用的剪贴板工具，同样不新增依赖。真写剪贴板的用例默认跳过，只在 `MONICA_CLIPBOARD_TEST=1` 下运行并从另一个进程读回比对；2026-09-22 这条用例在真实 Linux（WSL2 里的 xclip）上开着开关跑过并通过，Linux 的 `xclip -selection clipboard` 分支由此被本仓库代码真实发起过，`wl-copy`、`xsel` 与 macOS 的 `pbcopy` 仍然没有（见 [SECURITY.md](../SECURITY.md) 的「验证范围」）。

`D` 删除中选行复用同一套表单机制：确认字段是**空的**，绝不把目标名称预先填好让回车一路通过。`action()` 要求键入值与目标逐字相等，不相等就只是把表单留在屏幕上——那一刻既没有请求 broker 锁定，也没有打开保险库，正在服务的 AI 会话不受影响。名称背后是谁决定删什么：绑定了服务连接的凭据走连接删除（其授权一并撤销），普通条目与空分类各走自己的墓碑命令，非空分类在选择那一刻就报还差多少条目与子分类，不进入表单。按键条上的 `D` 与帮助页那一行同源，中英文各一份文案。

颜色按用户截图选择：背景 `#282c34`、蓝色选中 `#61afef`、青色路径 `#56b6c2`。绿、黄、红分别用于有效只读状态、写权限或锁定提醒、失效或错误状态。授权范围仍由真实数据决定，颜色不会赋予权限。

字符宽度、换行和截断按完整 grapheme 处理，保留中文与 emoji。预览和消息的滚动上限按换行后的内容计算。推荐 Nerd Font；`:icons` 或 `MONICA_TUI_ICONS=plain` 使用普通字符，操作含义始终有文字。

视图仅消费明确公开的名称、用途、仓库、允许操作、时间和路径。绘制、筛选、帮助和图标切换不会读取保险库、Token、主密码或客户端 capability。选中项仍先按可见行映射到名称/远端路径，再交给原有管理函数。

验收图来自 Ratatui TestBackend 的真实单元格，使用合成公开数据，并非原生终端截图；PNG 字体为本机安装的 0xProto Nerd Font Mono，中文回退到微软雅黑。终端字体和行距由使用者的终端决定。

多行的密钥字段（PEM 与 ASCII armor）整字段掩码：屏幕上只有 `*`，标签行末尾给出"已粘贴 N 行"来证明粘贴没被截断。`Enter` 在这种字段里插入换行，保存仍用 `Ctrl+S`。列表与预览只读公开摘要——算法与位数、指纹、注释或用户 ID、公钥分块数、是否含私钥——任何一帧都不绘制密钥字节，导出也不在界面里，只有 `monica keys export` 这一条人工命令。

弹窗清理背景时，会同时清理跨过左右边缘的完整 grapheme。仅清理矩形内部可能留下左边半个中文字符，使终端跳过随后的边框单元格。被切掉的字符保留背景色；关闭弹窗后正常重绘原页面。

PNG 中的 `─│╭╮╰╯` 全部由同一个字符网格绘制，共享中心线、线宽与单元格接点。不能混用字体圆角和手绘竖线，否则字体基线和行高的差异会造成断口。像素回归检查覆盖四角的八处接点；TUI 回归检查覆盖全部表单、帮助和长结果在连续缩放及长中文、组合字符、emoji 输入下的边框和重绘。

竖线不漂移靠三条规则：记录行按整栏宽度精确填满（两端各留一格给胶囊），所以选中行的胶囊挨着竖线是设计结果，不是错位；竖线最后绘制，`x` 只取自共享布局的列边界，与内容、语言、宽度都无关；回归测试从 70 到 140 逐列扫描两种语言，要求主页与设置页算出的两根竖线完全相同，并要求重绘帧与干净帧逐格一致。低于 70 × 20 时只渲染"终端过小"提示，不画竖线。

[WebDAV 登录弹窗（80 × 24）](images/webdav-login.png) · [四角放大图](images/border-detail.png)

开发时可在 Windows 上用 Python、Pillow 和上述字体重新生成验收图。渲染工具仅用于开发，不是 CLI 运行依赖：

```powershell
$env:MONICA_TUI_CAPTURE_DIR = Join-Path $env:TEMP 'monica-tui-captures'
cargo +1.97.0-x86_64-pc-windows-gnu test --lib tui::
python -m unittest discover -s scripts -p test_render_tui.py
python scripts/render_tui.py $env:MONICA_TUI_CAPTURE_DIR
```

`--font-dir` 与 `--system-font-dir` 可指定终端字体、系统回退字体目录。这里只导出测试构造的合成界面，不捕获用户保险库或终端会话。

Yazi 的 MIT 许可与署名保留在 [第三方说明](../THIRD_PARTY_NOTICES.md)。
