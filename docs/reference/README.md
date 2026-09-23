# Monica CLI 命令练习本

这是一个**教学站**，不是终端。它把 `monica` 的全部命令列出来、给出示例和真实输出，
并让你在浏览器里练手敲命令——只按同一份语法检查你敲的每个参数，**不会执行任何命令，
也碰不到任何凭据**。

- 三个视图：`总表`（搜索 + 分组 + 标记筛选）、`详情`（参数表、示例、实测输出、容易踩的地方）、
  `练习`（12 道关卡 + 自由输入模式）。
- 视觉走 Nothing 风格：黑灰面、单一红色强调、点阵标题字。右上角 `[ 暗 ]` / `[ 亮 ]` 切换主题，
  练习进度和主题都只存在本机 `localStorage`。

## 打开

本地直接双击 `index.html` 就能用（`file://` 下功能完整，只是标题字体走 Google Fonts CDN，
离线时退回系统等宽字体）。

部署到 GitHub Pages：仓库 `Settings → Pages → Build and deployment → Deploy from a branch`，
分支 `main`，目录 `/docs/reference`，站点地址即
`https://<org>.github.io/<repo>/docs/reference/`。这个地址本身尚未实测——目前所有验证都在本地
`file://` + 无头浏览器里完成，Pages 上线后的渲染需要部署后再看一次。

## 文件

| 文件 | 作用 |
| --- | --- |
| `index.html` | 页面骨架，三个视图都在同一页，用 hash 路由 |
| `site.css` | Nothing 风格样式与断点（860 / 560） |
| `site.js` | 渲染与交互，不含任何命令数据 |
| `data.js` | **生成物**：命令、参数、示例、实测输出、关卡 |
| `build.mjs` | 生成器，同时是校验器 |
| `examples.json` | 手写内容：中文摘要、分组、示例行、关卡、说明 |
| `grammar.js` | 语法引擎，浏览器和 `build.mjs` 共用同一份 |

## 更新流程

命令语法来自 `monica commands --json`，示例与关卡来自 `examples.json`。改了任何一边之后：

```bash
cargo build --release                       # 先有二进制
node docs/reference/build.mjs               # 重新生成 data.js
node docs/reference/build.mjs --check       # 只比对，不写文件
```

`build.mjs` 会拒绝生成不干净的数据：示例行、关卡答案都要能被当前语法接受，
每条语法内命令都得有教学条目，`tested: true` 的示例必须带实测输出，
`tested: false` 必须写明为什么没采。它还会打印哪些条目是手写的（目前只有 `keys export`，
它故意不出现在 `commands --json` 里）。

生成数据不需要在 CI 里装 node：`cargo test` 带一个
`teaching_site_data_matches_the_live_grammar`，直接把 `data.js` 和进程内的 clap 语法树比对
命令名与长选项集合，漂移就失败。（本仓库目前没有 CI 工作流，这个守卫靠本地 `cargo test` 生效。）

## 缩写与真解析器一致

练习模式不是「看着像就行」，它按 `grammar.js` 逐字判定，目标是和 `clap` 在每个缩写写法上给出
同样的接受／拒绝：

- 命令别名（`monica ck work`）、长选项别名（`--no-prompt`）；
- 全局参数放在命令名之前（`monica -j list`、`monica --lang en status`）；
- 值连着写（`-cwork`）、短选项堆叠（`-ws`）、`--flag=value`；
- 裸 `--` 之后一律算位置参数（`monica note work -- --json`）；
- 只要一个的 ArgGroup（`check` 的 `NAME` 或 `--client`，两个都给就冲突）、
  可以叠加的 ArgGroup（`keys edit --title --note`）、必须接子命令的分组（`webdav`）；
- 候选值连同别名与大小写（`language zh`、`language ZH-CN` 通过，`--provider GITHUB` 不通过）；
- 帮助短路：`--help`／`-h`（含 `-jh` 这种堆叠）出现在任何位置都只打印帮助，`keys help` 这类
  内建 `help` 词只在有子命令的命令上成立（`list help` 是多余位置参数），`--version`／`-V` 只在根上；
  帮助词**之前**的错误照报（`check --bogus -h` 仍失败），之后的检查全部跳过（`keys gpg n -h` 通过）。

一致性是量出来的，不是推出来的：把 `target/release/monica-pass.exe` 跑在临时目录的一次性配置上，
用退出码 2 和本地化解析报错判定「真解析器拒绝」，再和 `grammar.js` 的判定逐行比对。最近一轮
（0.5.0 二进制，Windows）：

| 扫描 | 行数 | 分歧 |
| --- | --- | --- |
| 全部示例行 + 关卡答案 | 131 | 0 |
| 全部裸命令与别名 | 95 | 0 |
| 生成的缩写写法（堆叠／连写／别名／前置全局／`--`） | 73 | 4（见下） |
| ArgGroup 与候选值 | 30 | 0 |
| 帮助短路（`-h`／`--help`／`help`／`--version`／与坏值的先后） | 59 | 3（见下） |

`examples.json` 里故意写错的示范行标 `"teachesError": true`，`build.mjs` 会要求语法**拒绝**它，
而不是像其他示例那样要求接受。

三处已知边界（页面上也写在 `练习` 顶部）：

1. 数字范围检查没做。`refresh --ttl abc`／`--ttl 70000` 真解析器会拒，练习本会放过——
   `commands --json` 里没有值类型／范围（clap 4.6 不公开 `ValueKind`），造不出来就不假装。
   候选值列表是公开的，所以那部分做了。
2. `monica tui --json` 这类「解析得过、CLI 自己拒绝运行」的组合，练习本在解析层就报 `no_json`，
   真二进制在运行层拒绝。两边都拒绝，只是拦下的层次不同，因此上面那一轮记为分歧而非缺陷。
3. `help` 后面再接 `--json`（`monica help -j`、`monica keys help -j`）同上：真 CLI 由应用层回
   `invalid_request`，练习本按「`help` 之后只能接命令名」拒绝。也是两边都拒绝、层次不同。

## 边界

- 页面上的「实测输出」全部来自发布版本的 release 二进制，跑在系统临时目录里的一次性保险库上，
  凭据是合成的假值；没有连接过任何真实账号。
- 练习模式给的是语法与意图判断，不代表这条命令在你的机器上会成功——权限、网络、
  保险库状态都不在这个站的范围内。
