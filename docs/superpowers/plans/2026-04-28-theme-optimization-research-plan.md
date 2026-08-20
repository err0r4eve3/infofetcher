# Theme Optimization Research And Design Plan

**Goal:** 基于优秀 RSS/阅读器产品与用户需求调研，优化 RSS Hub Reader 当前主题，让界面更像一个面向高吞吐阅读、长期自托管使用的专业阅读工具，而不是单纯换一组颜色。

## First-Principles Decomposition

1. **Real user goal:** 用户需要在大量订阅里更快筛选、更少疲劳、更清楚地知道当前状态，并能长时间阅读正文。
2. **Invariants:** 登录、订阅/文件夹、文章列表、正文阅读、搜索、未读/收藏筛选、摘要、OPML、主题持久化、可拖拽三栏宽度、移动端堆叠布局和 API 行为都不能改变。
3. **Constraints:** 现有前端是单个 React/Vite/TypeScript 应用；主题主要集中在 `apps/web/src/styles.css`，主题状态在 `apps/web/src/App.tsx`；不新增依赖，不改后端，不引入并行设计系统。
4. **Minimum viable change surface:** 以 CSS token 和现有组件状态为主；只在必要时给现有元素补充 `read/unread`、`active` 等 class，避免新组件和新交互。
5. **Evidence of completion:** 类型检查和构建通过；桌面/移动、亮色/暗色、登录/阅读/空状态/摘要/错误状态均经过浏览器 QA；正文和关键控件满足 WCAG AA 对比度目标。

## Research Sources

- Google Material dark theme guidance: 暗色主题应使用烟灰色表面、降低色彩饱和度，并通过多个 surface 层级表达空间关系。Source: https://design.google/library/material-design-dark-theme
- W3C WCAG 2.2 contrast guidance: 普通文本至少 4.5:1，大文本至少 3:1；不要只依赖色相判断可读性。Source: https://www.w3.org/WAI/WCAG22/Understanding/contrast-minimum
- Readwise Reader: 把文章、newsletter、RSS、PDF 等阅读材料集中到一个可自定义的阅读平台，强调高质量阅读、标注和外观调节。Source: https://readwise.io/read and https://docs.readwise.io/reader
- Feedly: 用户核心诉求是降噪、优先级、摘要、去重、过滤和团队沉淀，而不是更多装饰。Source: https://docs.feedly.com/article/523-getting-started-with-feedly
- NewsBlur: power user 价值集中在智能训练、全文过滤、实时抓取、键盘快捷键、多布局和快速浏览。Source: https://www.newsblur.com/features
- Inoreader: 成熟 RSS 产品同时服务入门用户和高级用户，关键词是可读界面、搜索发现、规则、标签、文件夹和 newsletter 汇入。Source: https://www.inoreader.com/
- Miniflux: 自托管 RSS 用户重视极简、键盘效率、隐私、无遥测和低维护成本。Source: https://miniflux.app/index.html
- RSS reader UI principles: RSS UI 的底线是简洁、一致、响应式、可发现反馈和适度自定义。Source: https://www.feedviewer.app/answers/rss-reader-user-interface-design-principles

## User Need Synthesis

- **扫描优先:** RSS 是信息流，不是静态文章页。用户先扫来源、标题、时间、未读状态，再决定是否进入正文。
- **降噪优先:** Feedly、NewsBlur、Inoreader 都在不同层面解决噪音。主题应减少装饰噪音，把视觉权重让给标题、未读、筛选和摘要状态。
- **长读舒适:** 正文区域要比列表更安静，文字对比足够但不能刺眼，行宽、行高和段落间距要稳定。
- **自托管信任感:** 这类用户重视控制权、隐私和可靠性。视觉语言应偏工具、新闻室、知识工作台，不应像营销页或社交信息流。
- **高效操作:** 键盘快捷键、筛选按钮、拖拽分栏和 OPML 操作都要看起来“可用”，但不抢阅读内容。
- **移动端务实:** 移动端不是完整桌面替代，目标是可扫描、可选文章、可读正文，不追求复杂多栏。

## Current Theme Audit

### What Already Works

- 主题已 token 化，亮色和暗色都在 `styles.css` 的 `:root` / `:root[data-theme="dark"]` 中集中定义。
- `App.tsx` 已有主题持久化、系统偏好 fallback 和登录页/阅读页复用的 `ThemeToggle`。
- 三栏布局、可拖拽宽度、键盘分栏调整、移动端单列堆叠已经存在。
- 焦点态和 `prefers-reduced-motion` 已覆盖基础可访问性。

### Gaps To Fix

- **亮色主题偏“绿色后台管理”。** 当前 light tokens 大量使用 sage/green，气质接近环保工具或后台表单，不够像新闻/知识阅读器。
- **暗色主题存在双品牌感。** 暗色主 accent 是 Material blue，但品牌仍是绿色，摘要、选中、info 状态混在一起后辨识成本变高。
- **未读状态不够强。** 文章是否未读主要靠小图标，扫 80 篇文章时不够快。需要用标题字重、左侧短线或背景层级强化，但不能把列表做花。
- **摘要和状态面板过于抢戏。** 当前 summary/status 使用 accent surface，容易和 selected row 竞争注意力。摘要应该像“右侧辅助批注”，不是主内容。
- **侧边栏设置区视觉权重偏高。** 推荐订阅、添加订阅、OPML 都很有用，但日常阅读时应退到次级层级。
- **正文行宽和阅读层级还能更稳。** 当前正文 900px max-width 对长段英文/中文混排略宽；标题、元信息、feed chip、正文之间可更接近出版物节奏。

## Proposed Direction

**Aesthetic:** Quiet Newsroom Utility. 新闻室/知识工作台，冷静、密集、可信，不做社交流光感。

**Decoration:** Minimal. 只用表面层级、细线、字重、少量 accent 表达状态。

**Layout:** Keep current three-pane grid. 它符合 RSS 类产品的 table-stakes：来源、列表、正文同时可见。优化重点是层级和密度，不重做 IA。

**Typography:** 保留现有系统字体栈，不新增字体依赖。通过字号、行高、字重和 tabular number 调整阅读感。

**Motion:** Minimal-functional. 保留 hover/focus 反馈，避免大面积转场。读者的注意力应停在内容上。

## Proposed Token Strategy

### Light Theme

- App background: `#f5f7f6`
- Shell background: `#eceff0`
- Sidebar: `#f8faf9`
- Panel: `#ffffff`
- Reader: `#fcfcfb`
- Surface muted: `#f1f4f3`
- Border: `#d9dfdc`
- Strong text: `#111917`
- Body text: `#1f2a27`
- Muted text: `#61706a`
- Primary accent: `#167760`
- Primary accent strong: `#0f5f4c`
- Primary accent soft: `#e0f2ec`
- Info: `#315f8c`
- Danger: `#a33a32`

### Dark Theme

- App background: `#101312`
- Shell background: `#141716`
- Sidebar: `#181c1b`
- Panel: `#171b1a`
- Reader: `#101312`
- Surface: `#202522`
- Surface muted: `#252b28`
- Border: `#313a36`
- Strong text: `#edf2ee`
- Body text: `#dfe6e1`
- Muted text: `#a9b4af`
- Soft text: `#7f8d87`
- Primary accent: `#8bd8c2`
- Primary accent strong: `#b4f1dc`
- Primary accent soft: `#15332c`
- Info: `#a8c7fa`
- Danger: `#ffb4a8`

Decision: keep green/teal as the primary brand accent in both themes. Use blue only for informational chips or links. This removes the current blue-vs-green brand split.

## Implementation Milestones

- [x] **Milestone 1: Lock theme tokens.** Refine light/dark token values in `apps/web/src/styles.css`; keep variable names stable so component CSS churn stays small.
- [x] **Milestone 2: Improve scan hierarchy.** Add clearer unread/read/active list styling with typography and subtle structural cues. If needed, update `ArticleRow` class names in `apps/web/src/App.tsx`; no API changes.
- [x] **Milestone 3: Quiet the setup/sidebar areas.** Make add forms, presets, OPML, badges, and folder rows visually secondary during normal reading while preserving discoverability.
- [x] **Milestone 4: Rebalance reader and summary surfaces.** Reduce summary/status panel dominance, narrow long-form reading measure, tune title/meta/chip hierarchy.
- [x] **Milestone 5: Mobile and interaction polish.** Verify toolbar wrapping, article list height, selected states, focus rings, resizers, and button text do not overlap at 390px width.
- [x] **Milestone 6: Validation and screenshots.** Run deterministic checks and browser QA before signoff.

## Acceptance Criteria

- Light theme reads as neutral professional newsroom/tooling, not green-tinted admin UI.
- Dark theme has clear Material-style surface levels without pure black/pure white glare.
- Primary accent is consistent across light and dark; semantic colors are visually distinct.
- Unread, read, hover, selected, favorite, summary, error, disabled, and focus states are distinguishable without relying only on hue.
- Body text and key controls meet WCAG AA contrast targets: normal text >= 4.5:1, large text/UI indicators >= 3:1.
- Article body line length stays comfortable on desktop and does not force horizontal scrolling on mobile.
- Existing behavior remains intact: theme persistence, article selection, filters, summaries, OPML, resizable panes, keyboard shortcuts.
- No new dependencies, backend changes, routes, configs, or new design-system files are required for this pass.

## Validation Commands

```bash
npm run typecheck:web
npm run build:web
```

Browser QA inventory:

- Login screen in light and dark mode.
- Authenticated reader at desktop viewport, e.g. 1440x900.
- Authenticated reader at mobile viewport, e.g. 390x844.
- Article list with read, unread, selected, favorite, summary flags, long titles, long excerpts.
- Reader pane with summary pending, summary complete, summary failed, fallback content notice, and long article body.
- Sidebar with many folders, long feed names, preset groups, OPML actions, and empty metrics.
- Keyboard focus for search, toolbar buttons, feed rows, article rows, reader actions, theme toggle, and pane resizers.

## Risks And Tradeoffs

- **Taste risk:** Color and density are subjective. Mitigation: compare screenshots of current vs optimized light/dark before merging.
- **Density risk:** Stronger unread hierarchy can make the list feel busier. Mitigation: use no more than two cues per state, likely weight plus subtle left rail.
- **Contrast risk:** Muted text can pass in one surface and fail on another. Mitigation: test actual token pairs instead of eyeballing.
- **Scope risk:** Adding density controls or reading preferences would be useful, but it is not required for theme optimization. Defer unless the implementation exposes a real usability gap.

## Current Status

- Research and design planning completed on 2026-04-28.
- Implementation completed on 2026-04-28.
- `npm run typecheck:web` passes.
- `npm run build:web` passes.
- Key light/dark foreground/background token pairs pass WCAG AA contrast checks at >= 4.5:1.
- Browser QA used a local mocked API server and Playwright CLI.
- Verified desktop light reader, desktop dark reader, dark failed-summary state, mobile dark reader, login light/dark, theme persistence after reload, login/logout flow, no mobile horizontal overflow, and no browser console warnings/errors.
- Screenshot artifacts: `output/playwright/theme-desktop-light.png`, `output/playwright/theme-desktop-dark.png`, `output/playwright/theme-desktop-dark-failed-summary.png`, `output/playwright/theme-mobile-dark.png`, `output/playwright/theme-login-mobile-dark.png`, `output/playwright/theme-login-mobile-light.png`.
- Published to homeserver production at `https://rss-reader.error-forever.com` on 2026-04-28.
- Production build assets verified: `assets/index-CiRFPTwc.css` and `assets/index-BVTGYzLu.js`.
- Production verification passed: public `/api/health`, public HTML asset references, CSS/JS asset headers, authenticated `/api/metrics`, and browser login-page render.
