# 前端设计 loop 台账

目标：参考 Taste Skill 和至少 5 份 GitHub 前端设计 skill，分小步整体优化 `~/.claude/skills/error-design-language` 和本项目前端。每轮只做一项，做完验证、记录、再进入下一轮。

开始：2026-10-20。

## 参考来源

| 来源 | 版本 | 取用的部分 |
|---|---|---|
| Leonxlnx/taste-skill | 2026-10 | 工作台规则、数据自洽、截图评审（已吸收，见 skill `taste-v2-app-screens.md`） |
| pbakaus/impeccable | 4.7.0 | 四种模式（说服、操作、阅读、体验）；精修保留与重设计替换；craft-floor 的验证项和拒绝清单；浏览器自带表面（选区、光标、滚动条、焦点环、数字）；明暗按使用场景选 |
| anthropics/skills · frontend-design | 2026-10 | 先写计划再对照简报复查；当前 AI 页面的 5 类聚集特征（中点串联元信息、链接后加 →、近黑代替黑、等宽小标签等）；文案规则（同一动作全流程同名） |
| vercel-labs/web-interface-guidelines | 2026-10 | MUST/SHOULD 审计清单：焦点、命中区、表单、URL 反映状态、`transition: all`、`…` 字符、Intl 日期、`theme-color`、原生 select 配色 |
| ibelick/ui-skills · baseline-ui | 2026-10 | 交互反馈不超过 200ms；固定 z-index 刻度；`h-dvh`；空状态给一个下一步；每个视图一个强调色 |
| jakubkrehel/make-interfaces-feel-better | 2026-08 | 同心圆角；按下 `scale(0.96)`；图标描边匹配字重；图片 1px 纯黑/纯白描边；评审输出格式（严重度、位置、前后对比、考虑过但放弃） |
| Dammyjay93/interface-design | 2026-06 | 一屏一个焦点；字号比例 1.2–1.25，字重和颜色比字号更有效；深色层级每级只差几个百分点亮度；侧栏与画布同色；输入框比周围略暗；深度策略只选一种；文字分四级 |
| nextlevelbuilder/ui-ux-pro-max-skill | 2026-10 | 检查优先级（可访问性 → 触控 → 性能 → 风格 → 布局 → 字体配色 → 动效 → 表单 → 导航 → 图表）；密度与动效“旋钮” |
| emilkowalski/skills | 2026-10 | 动效决策顺序（本机已装 animate、apple-design 等技能，按需调用） |

本地副本只在 `/tmp/design-refs`，不进仓库。

## 待办（一轮一项，S = skill，P = 项目）

- [x] S1 skill 入口加“先判断模式”和“精修保留／重设计替换，不折中”（Impeccable）。
- [x] P1 浏览器自带表面：`::selection`、`caret-color`、滚动条、`<meta name="theme-color">`、字体平滑。
- [x] S2 新建 `references/craft-checklist.md`：合并 Vercel MUST、craft-floor、baseline-ui、MIFB 中与现有规则不重复的条目，按严重度分级，附评审输出格式。
- [x] P2 机械审计：`transition: all`、`...` 与 `…`、日期是否走 Intl、计数是否 `tabular-nums`、z-index 是否成刻度。
- [x] S3 反 AI 痕迹清单补 Anthropic 的聚集特征和 Impeccable 的拒绝清单（模态优先、>1px 彩色侧边条、按类别选明暗）。
- [ ] P3 评估事件卡片的中点串联元信息和行尾箭头，确认是否带信息，再决定去留。
- [ ] S4 `visual-system.md` 补深色层级规则（每级几个百分点亮度、侧栏同画布、输入框略暗、深度策略只选一种、文字四级）。
- [ ] P4 按 S4 复查本项目深色和浅色的层级、侧栏、输入框。
- [ ] P5 命中区（桌面 ≥40px、触屏 ≥44px）与图标描边匹配字重。
- [ ] P6 动效审计：时长、按下反馈统一、可中断（transition 而非 keyframes）、减少动态效果。
- [ ] P7 空状态和错误状态：每个都给一个下一步。
- [ ] S5 做减法：SKILL.md 与各 reference 去重，确认没有互相矛盾的数值。

## 每轮规则

- 只做一项。项目改动要截图验证（桌面、手机，浅色、深色）、跑测试和构建；对比线上与 HEAD 后用 `deploy-web.sh` 上线；本地提交，不推送。
- skill 改动只改 `~/.claude/skills/error-design-language`，不新增并行规则；与用户偏好冲突的外部规则不采用，写明原因。
- 每轮在下面记一行：做了什么、证据、遗留。

## 记录

| 轮 | 项 | 结果 | 证据 | 遗留 |
|---|---|---|---|---|
| 1 | S1 | SKILL.md 入口改为 6 步：第 1 步写明重设计整体替换、不折中；新增第 2 步四种模式与“明暗按场景选” | 读回 SKILL.md 第 12–26 行 | 导语段仍写“都适用黑白灰”的历史原话，留给 S5 统一 |
| 2 | P1 | theme-color 随手动切换的主题更新（原为旧近黑 #090a0c）；光标用强调色；滚动条按主题配色。选区与字体平滑原已有 | 浏览器实测切换前后 meta、caret、scrollbar-color 计算值；14 测试、构建通过；公网 index.html 已是新值；回滚 `before-webloop-p1-20261019T190404Z` | 深色 --app-bg 0.215 折合 #171a1c，仍偏暗，P4 再评估 |
| 3 | S2 | 新建 `references/craft-checklist.md`（8 节：交互与状态、表单、文字、浏览器表面、视觉细节、动效、性能、评审输出格式），只收现有 reference 没有的条目；SKILL.md 与 README 加链接 | 与 anti-slop、verification 逐条对照去重；按下缩放等冲突数值沿用旧规则 | 清单里的 z-index 刻度、Intl 日期等在本项目的落实交给 P2 |
| 4 | P2 | 13 个散乱 z-index 收成 6 级 token（raised/popover/sticky/status/floating/skip，保持原相对顺序）；2 处 `...` 改 `…`；日期格式化器复用单例。`transition: all` 0 处、日期已走 Intl、计数与时间已是 tabular-nums，无需改 | 浏览器实测弹层、面板菜单、手机吸顶与底栏层级；tabular-nums 计算值；14 测试、构建通过；回滚 `before-webloop-p2-20261019T191457Z` | 无 |
| 5 | S3 | anti-slop 第 1 节补 4 条拒绝项（硬偏移阴影、符号代图标、占位小图表、无谓模态）和 Anthropic 五类聚集特征，附“换同类简报再做一遍”的判断法；中点、箭头、等宽改为“带信息就留”；加“表现力只花在一处”；第 2 节深色底改为石墨灰 | 与现有条目逐条对照，彩色侧边条、eyebrow、01/02 已存在未重复 | 本项目中点元信息和行尾箭头的去留由 P3 按“带信息就留”判断 |
