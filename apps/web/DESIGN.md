---
name: Infofetcher
description: A calm, verifiable signal desk for the world's meaningful changes.
colors:
  graphite-canvas: "oklch(0.962 0.004 250)"
  signal-surface: "oklch(0.995 0.002 250)"
  quiet-surface: "oklch(0.952 0.004 250)"
  graphite-text: "oklch(0.34 0.008 250)"
  graphite-strong: "oklch(0.24 0.006 250)"
  graphite-muted: "oklch(0.45 0.008 250)"
  signal-ink: "oklch(0.52 0.16 258)"
  signal-on-ink: "oklch(0.99 0.004 258)"
  signal-soft: "oklch(0.945 0.03 258)"
  signal-border: "oklch(0.74 0.09 258)"
  confirm-green: "oklch(0.47 0.11 155)"
  confirm-soft: "oklch(0.95 0.035 155)"
  correction-red: "oklch(0.47 0.12 26)"
  correction-soft: "oklch(0.96 0.025 26 / 0.94)"
typography:
  display:
    fontFamily: "Avenir Next, PingFang SC, Hiragino Sans GB, Microsoft YaHei, ui-sans-serif, system-ui, sans-serif"
    fontSize: "38px"
    fontWeight: 600
    lineHeight: 1.08
    letterSpacing: "-0.03em"
  headline:
    fontFamily: "Avenir Next, PingFang SC, Hiragino Sans GB, Microsoft YaHei, ui-sans-serif, system-ui, sans-serif"
    fontSize: "28px"
    fontWeight: 600
    lineHeight: 1.15
    letterSpacing: "-0.025em"
  title:
    fontFamily: "Avenir Next, PingFang SC, Hiragino Sans GB, Microsoft YaHei, ui-sans-serif, system-ui, sans-serif"
    fontSize: "20px"
    fontWeight: 600
    lineHeight: 1.27
    letterSpacing: "-0.025em"
  body:
    fontFamily: "Avenir Next, PingFang SC, Hiragino Sans GB, Microsoft YaHei, ui-sans-serif, system-ui, sans-serif"
    fontSize: "14px"
    fontWeight: 400
    lineHeight: 1.65
    letterSpacing: "normal"
  label:
    fontFamily: "Avenir Next, PingFang SC, Hiragino Sans GB, Microsoft YaHei, ui-sans-serif, system-ui, sans-serif"
    fontSize: "12px"
    fontWeight: 600
    lineHeight: 1.2
    letterSpacing: "normal"
rounded:
  page: "0"
  row: "4px (rows, tags, badges)"
  control: "6px (buttons, inputs, list containers)"
  panel: "10px (overlays only)"
  shell: "0"
  pill: "999px"
spacing:
  xs: "4px"
  sm: "8px"
  md: "12px"
  lg: "16px"
  xl: "24px"
  2xl: "28px"
components:
  button-primary:
    backgroundColor: "{colors.signal-ink}"
    textColor: "{colors.signal-on-ink}"
    typography: "{typography.label}"
    rounded: "{rounded.row}"
    padding: "0 15px"
    height: "44px"
  button-quiet:
    backgroundColor: "{colors.quiet-surface}"
    textColor: "{colors.graphite-muted}"
    typography: "{typography.label}"
    rounded: "{rounded.row}"
    padding: "0 11px"
    height: "44px"
  event-card:
    backgroundColor: "{colors.signal-surface}"
    textColor: "{colors.graphite-text}"
    rounded: "0 within a flat event list"
    padding: "18px 16px"
  revision-badge:
    backgroundColor: "{colors.signal-soft}"
    textColor: "{colors.graphite-strong}"
    typography: "{typography.label}"
    rounded: "{rounded.row}"
    padding: "0 8px"
    height: "24px"
---

# Design System: Infofetcher

## Overview

**Creative North Star: "信号台"**

这个系统像一座纪律严明的信号台：持续接收噪声，但只把经过排序、聚合和核验的变化送到用户面前。它冷静、可信、敏锐，信息密度高却不制造压迫感；每个层级都帮助用户更快完成判断，而不是延长停留时间。

界面采用 Linear 式的克制和清晰，但不复制其品牌装饰。结构由大面积石墨中性色、精确排版、有限状态色和可完成的事件列表构成。它明确拒绝 PRODUCT.md 中的“新闻门户式的信息轰炸和无限瀑布流”。

**Key Characteristics:**

- 有限、可完成的事件简报，而非无尽内容流。
- 事件、变化和证据状态始终先于来源文章。
- 高密度建立在强层级、稳定间距和清晰状态之上。
- 默认安静；只有确认、纠正、争议和焦点状态获得视觉强调。
- 桌面与移动端都保持完整键盘、对比度和触控可达性。

**The Signal Desk Rule.** 每个屏幕都必须让用户更快辨认真正的新变化；不能帮助判断的信息不应争夺主层级。

## Colors

“石墨信号”以冷静的灰阶承担结构，一种蓝色（Signal Ink）标示主操作与当前选中，绿/红只表达确认与纠正。

### Primary

- **Signal Ink** (`signal-ink`): 主操作、导航下划线、当前选中（淡蓝底 + 左侧蓝条）。确认状态改用 `confirm-green`。
- **Signal on Ink** (`signal-on-ink`): 只用于深色主操作上的高对比文本与图标。
- **Signal Soft** (`signal-soft`): 活动导航、命中标签和低强度选择状态。

### Neutral

- **Graphite Canvas** (`graphite-canvas`): 页面底色，提供低眩光的冷灰环境。
- **Signal Surface** (`signal-surface`): 事件卡、检索框和详情面板的主承载面。
- **Quiet Surface** (`quiet-surface`): 次级控制、摘要区和悬停反馈。
- **Graphite Strong** (`graphite-strong`): 标题、核心结论和关键数字。
- **Graphite Text** (`graphite-text`): 默认正文。
- **Graphite Muted** (`graphite-muted`): 元数据、解释和辅助标签。

### Tertiary

- **Correction Red** (`correction-red`): 仅用于错误、纠正、否认和争议等需要警觉的状态。
- **Correction Soft** (`correction-soft`): Correction Red 的低强度背景，不承担普通装饰任务。

**The Graphite Signal Rule.** 黑白灰承担结构；状态色必须对应可解释的数据语义，禁止纯装饰性彩色。

**The Rarity Rule.** Signal Ink 在任一视口中只能强调当前最重要的操作或状态，不能同时填满多个区域。

**The Red Means Correction Rule.** 红色只表示错误、纠正、否认或争议；禁止用于增长、热点或促销。

## Typography

**Display Font:** Avenir Next（回退至 PingFang SC、系统无衬线）

**Body Font:** Avenir Next（回退至 PingFang SC、系统无衬线）

**Label Font:** Avenir Next（回退至 PingFang SC、系统无衬线）

**Character:** 单一人文无衬线体系保持技术精度与中文可读性。层级靠字号、字重和留白建立，不靠字体混搭制造戏剧性。

**The Matched Weight Rule.** PingFang SC 最重为 600，Avenir Next 在 650 以上会切到 Bold/Heavy，使中文里的数字和英文明显发黑。强调字重统一用 600，不使用 650–850 的中间值。

### Hierarchy

- **Display**（600，38px，1.08）：页面主命题，只允许每个主视图出现一次；移动端收敛至 32px。导航已标明当前视图时，页面标题只保留给读屏器。
- **Headline**（600，28px，1.15）：事件详情标题和关键区域标题。
- **Title**（600，20px，1.27）：事件卡核心结论，最多占三行前的主要阅读焦点。
- **Body**（400，14px，1.65）：摘要、变化解释与证据文本；长文本控制在约 72ch 内。
- **Label**（600，12px，1.2）：时间预算、状态、元数据和紧凑操作；使用短语，不使用整段大写英文。

**The One Hierarchy Rule.** 同一屏幕只允许一个 Display 层级；卡片标题必须服从页面命题。

**The Evidence Legibility Rule.** 证据、未知项和分歧不得使用低于 12px 的字号，也不得仅靠低对比度隐藏。

## Elevation

系统采用色调分层为主、受控阴影为辅的混合策略。静态内容面默认扁平，通过背景明度和 1px 内描边分层；阴影只服务于粘性顶栏、浮动状态、登录面板等真正脱离文档流的表面。

主工作区固定为三级窗口关系：`Layer Recessed` 承载进度与筛选槽，`Layer Work` 承载连续事件列表，`Layer Focus` 承载当前事件详情。浅色模式用明度阶梯配合短阴影，深色模式主要依靠表面明度递增；只有详情检查器和临时状态允许更完整的环境阴影。

### Shadow Vocabulary

- **Panel Float** (`--shadow-panel`): 仅用于粘性、浮动或需要跨越内容层的面板；浅色模式为宽而低对比的环境阴影，深色模式提高遮蔽度。
- **Window Focus** (`--shadow-window`): 仅用于当前焦点窗口；事件详情比事件列表高一个层级，但不制造悬浮卡片墙。
- **Surface Rest** (`--shadow-surface`): 工作面与搜索入口的短阴影，负责从画布中分离，不表达可拖拽或浮动。
- **Control Lift** (`--shadow-control`): 当前筛选与次操作按钮的轻微抬升；深色实心与 `--shadow-primary` 只属于主操作。
- **Row Lift** (`--shadow-row`): 仅用于短暂悬停、主要操作或被提升的单行表面。
- **Inset Boundary** (`inset 0 0 0 1px`): 事件卡、分段控件和证据容器的默认结构边界。

**The Flat-at-Rest Rule.** 内容卡在静止状态禁止使用外投影；先用色调和内描边建立边界。

**The Honest Elevation Rule.** 只有真实覆盖其他内容或响应交互的元素可以抬升；阴影不能替代层级设计。

**The Glass-on-Overlay Rule.** 背景模糊只用于内容会从下方滚过或被覆盖的表面：吸顶标题、导航、浮动状态和划词翻译。登录面板、工具栏这类不浮动的表面用实色 `Layer Focus` / `Layer Work`。

**The One-Border Rule.** 从画布向内的任一视线最多穿过一层闭合边框；带边界的容器内部改用分隔线、留白或无框按钮。

## Components

### Buttons

- **Shape:** 普通按钮使用 6px，核心触控高度固定为 44px；10px 只用于输入与组合控件，静止态工具栏按钮优先透明而不是显示一排圆角底板。
- **Primary:** Signal Ink 背景、Signal on Ink 文本，水平内边距 15px，字重 600。
- **Selected:** 导航与工作流标签使用 2px Signal Ink 下划线；主题、覆盖等筛选片使用 Signal Soft 底 + Signal Ink 文字，不加下划线。
- **Secondary:** Layer Work 背景配合 Control Lift；低频反馈和辅助动作停留在 Layer Recessed。
- **Hover / Focus:** 悬停只改变色调；键盘焦点使用 2px Signal Ink 外轮廓和 2px 偏移。
- **Secondary / Quiet:** Quiet Surface 或透明背景，不得与主操作争夺对比度。

### Chips

- **Style:** 胶囊形（999px），高度 24px，12px 加粗标签；默认使用 Signal Soft 或语义状态软背景。
- **State:** 选中态必须同时改变背景与文字对比；纠正和争议使用 Correction Red 体系，并保留文字标签。

### Cards / Containers

- **Corner Style:** 页面、连续工作区和相连窗格使用 0px；当前焦点窗格与内容分组最多使用 6px；输入与组合控件使用 10px；16px 只允许用于会覆盖页面的弹层。
- **Background:** Signal Surface 承载内容，Quiet Surface 承载内部摘要或次级证据。
- **Shadow Strategy:** 页面层级优先使用背景明度步进和 1px 分隔线；只有当前焦点窗格与真实浮层可以使用外投影。
- **Internal Padding:** 事件卡使用 18px × 16px；详情内容使用 24px。

### Inputs / Fields

- **Style:** 独立字段为 44px 高、10px 圆角、Quiet Surface 背景；紧凑顶栏搜索使用 6px；大型提问入口本身就是输入控件，不再套 16px 卡片容器。
- **Focus:** 边界切换为 Signal Ink，并增加 4px 低对比焦点光环。
- **Error / Disabled:** 错误使用 Correction Red 文本、软背景和边界；禁用态降低至 48–55% 不透明度并取消指针反馈。

### Navigation

- **Style:** 桌面顶栏排列主视图，单项最小高度 44px；默认 Graphite Muted，活动项使用 2px 下划线，不显示圆角底板。
- **Mobile:** 固定底部四项导航，所有项目保持 44px 可点击高度和等权布局；文字标签不得仅以图标替代。

### Event Card

事件卡是系统的签名组件。它按“序号与状态 → 事件结论 → 新变化与评分 → 关注命中”组织：元数据行给出修订类型、时间与可信标签；摘要最多两行；页脚一行并列变化原因（why_now）与综合/可信/来源分数。“影响与边界”只在详情面板展开，不在每张卡片重复可信度结论。选择后在邻近详情面板中展开时间线和证据，不在列表内无限扩张。

卡片是 `article`，标题是 `h3` 内的按钮，按钮伪元素覆盖整张卡片作为点击区域；读屏器只朗读标题，标题仍可按标题导航。选中态用 `aria-current`。快讯行采用同样结构，只有纠正与争议保留红色边界。

**The One Decision per Component Rule.** 每个控件只承担一个清晰动作；卡片本身负责选择事件，内部不堆叠一排同权按钮。

### Brief Workflow

报告工作流把半日/全天报告内的有限事件分为“待处理、稍后、已完成、全部”四个稳定状态，并持续显示本轮处理进度。完成状态由服务端反馈持久化；稍后项目在浏览器本地保存事件快照，使用户跨时间窗回看时仍能进入最新详情。

当可见报告包含多个主题时，显示带事件数量的主题目录；目录使用原生页内链接，跳转后焦点落在对应主题标题，移动端自然换行。事件序号按分组后的页面阅读顺序连续排列；目录与序号随状态筛选更新，空列表或单一主题不显示目录。

默认详情选择分组后的首个可见事件，显式事件链接优先。选中事件以焦点表面和左侧标线区分于悬停。单栏布局下，点击事件将滚动与键盘焦点移到详情；详情提供返回列表操作，恢复到原事件，原事件已被筛掉时回到列表区域。首屏说明与进度保持紧凑，为事件内容留出阅读空间。

圆角由外层容器承担：报告主题列表外缘、设置分组、摘要与来源分组统一使用 6px；列表内部事件行保持直角，避免每行重复圆角。输入与组合控件保留 10px，16px 留给登录或覆盖页面的面板。

**The Finite Report Rule.** 每份报告必须绑定 12h/24h 覆盖窗口、证据门槛和有限入选集合；进度只计算完成或明确推迟的事件，不能用滚动深度冒充完成。

## Do's and Don'ts

### Do:

- **Do** 把每份报告限制为明确的半日/全天窗口和有限事件数量，并按稳定主题分节。
- **Do** 使用 44px 最小交互高度、2px 可见焦点轮廓和文字状态标签。
- **Do** 先展示事件结论、新变化、证据状态和未知项，再提供原始文章入口。
- **Do** 使用 Signal Surface、Quiet Surface 和 1px Inset Boundary 建立高密度但可扫描的层级。
- **Do** 用 Canvas → Recessed Workspace → Focus Pane 的背景步进表达页面层级，让圆角只承担交互或浮层语义。
- **Do** 在 `prefers-reduced-motion` 下取消位移、旋转和非必要过渡，同时保留状态变化。
- **Do** 在输入框的 Enter / Escape 处理中先判断 `isComposing`，避免中文输入法选词时误提交或清空。
- **Do** 标题用 `text-wrap: balance`，正文用 `text-wrap: pretty`，避免末行孤字。
- **Do** 同一数字只用一个名词：卡片 meta 只显示可信等级，来源数只在来源行出现。报告卡片按时间窗统计来源，详情按事件累计统计，详情里写明“累计”。
- **Do** 订阅源名称显示短名（`Section | Brand` 取末段，`Brand - 口号` 取首段），完整名称放进 `title`。
- **Do** 计数在数据到达前不显示，不用 0 占位；徽标数字必须是它旁边标签的数量。

### Don't:

- **Don't** 在主导航旁重复产品名、账号或功能说明；页面标题和导航已经提供定位。
- **Don't** 做成“新闻门户式的信息轰炸和无限瀑布流”；任何主视图都必须有可完成的边界。
- **Don't** 用彩色热点标签、夸张数字、轮播横幅或大面积警示色伪造重要性。
- **Don't** 让来源文章标题压过事件结论，也不要把文章数量误当作事件重要性。
- **Don't** 给静态事件卡添加外投影；如果卡片看起来漂浮，层级就已经过度。
- **Don't** 给相连窗格、正文或连续列表中的每一行重复添加大圆角；结构层与内容层必须使用不同的形状语言。
- **Don't** 把进度、摘要、证据行和表单区默认包装成圆角卡片；标题、留白和分隔线应先承担分组职责。
- **Don't** 用颜色单独表达确认、纠正、争议或选中状态；必须同时提供文本或结构差异。
