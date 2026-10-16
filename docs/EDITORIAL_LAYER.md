# 编辑层（借鉴 Horizon，2026-10-16）

## 问题

简报排序的“影响”分来自中英文关键词表（`world.rs::impact_signal`），占总分 28%。它分不清“某国选举结果”和“某公司内部选举”，也无法说明为什么重要。信源质量分是手填常量，没有反映实际产出。

## 范围

| # | 能力 | 落点 |
|---|---|---|
| 1 | LLM 评审：0–10 分、理由、一句话摘要、标签 | `editorial.rs` 后台任务，结果存 `event_assessments` |
| 2 | 编辑 Profile：`world-news`、`tech-research`、`long-read`，各有路由说明和评分标准 | `apps/api/profiles/<id>/*.md`，编译期载入；信源可固定 Profile |
| 3 | 此前进展：在过往评审摘要上做 BM25（中文 bigram），候选交给同一次评审调用判断是否直接相关 | 评审输出 `history_note` 与引用 |
| 4 | 栏目配额：每个栏目最多占简报条数的固定比例 | `select_diverse` 硬上限，`BRIEF_SECTION_SHARES` 可调 |
| 5 | 社区讨论：HN 事件抓前 5 条评论，生成一到两句带归属的观点 | 评审输出 `community_note` |
| 6 | 信源画像：30 天文章数、评审数、通过数、信噪比、均分，自动微调质量分，信噪比腰斩时告警 | `feed_quality_stats`，每 6 小时计算 |
| 7 | 日报：每天定时生成一期，代码渲染 Markdown，提供 Atom 订阅和 Webhook 推送 | `brief_editions`，`/feeds/brief.xml` |
| 8 | MCP：简报、快讯、搜索、事件详情、日报 | `POST /mcp`，`MCP_API_TOKEN` |

## 关键决策

- 路由和打分合并成一次调用，每批 6 个事件，共用一份 Profile 目录和评分标准。理由：省 token，同批比较还能让分数更一致。批 8 个时实测出现过输出截断。
- 只评审通过宽松预门槛的候选（多源、命中关注或关键词总分 ≥ 50），按优先级排队，每天上限 `EDITORIAL_DAILY_EVENT_LIMIT`（默认 600 个事件，约 100 次调用）。2026-10-16 实测：24 小时约 1,220 个事件过预筛；每批约 8–10k 输入、5–8k 输出 token。
- 只在独立来源数跨档（1 / 2 / 3–4 / 5+）或出现纠正、矛盾时重新评审，且距上次至少 6 小时。评审与简报选同一篇代表文章。
- 证据层不动：事件摘要仍走原有证据约束管线。评审结果只影响排序、过滤和“为什么重要”的文案，不进入摘要证据。
- 历史、社区观点和长文启示由同一次评审生成，不另起调用。
- 有评审时，影响分取 `score × 10`；低于 Profile 阈值的事件不进简报（命中关注的事件例外）。没有评审时退回关键词分，保证 LLM 不可用时简报照常工作。
- 自动质量微调只叠加在排序用的来源质量上（±8），不改手填的 `feeds.quality_score`，不影响快讯的 ≥70 准入门槛。

## 验收标准

- 单元测试覆盖：采样、BM25 排序与排除规则、配额、评审结果校验（未知 ID、越界分数、伪造历史引用）、信噪比微调、Markdown/Atom 渲染、MCP 协议。
- 生产：真实评审调用成功写入，简报 API 返回 `editorial_*` 字段，前端可见；Atom 地址公网可订阅；MCP `tools/list` 与 `tools/call` 用真实请求返回 200。

## 配置

| 变量 | 默认 | 说明 |
|---|---|---|
| `EDITORIAL_ENABLED` | `false` | 开启评审后台任务 |
| `EDITORIAL_INTERVAL_SECONDS` | `600` | 扫描间隔 |
| `EDITORIAL_DAILY_EVENT_LIMIT` | `600` | 24 小时内最多评审的事件数 |
| `EDITORIAL_THRESHOLDS` | `world-news:5,tech-research:5,long-read:5` | 各 Profile 进入简报的最低分 |
| `BRIEF_SECTION_SHARES` | `world:0.45,technology:0.3,business:0.3,security:0.2,general:0.25` | 栏目占简报条数的上限比例 |
| `BRIEF_EDITION_HOUR` / `BRIEF_EDITION_UTC_OFFSET_HOURS` | `8` / `8` | 每日生成时间（本地时区） |
| `BRIEF_FEED_TOKEN` | 空 | 设置后开放 `/feeds/brief.xml?token=` |
| `BRIEF_WEBHOOK_URL` / `BRIEF_WEBHOOK_FORMAT` | 空 / `generic` | 每期推送，`feishu` 用飞书文本消息格式 |
| `BRIEF_TELEGRAM` | `false` | 每期推送到已配置的 Telegram |
| `MCP_API_TOKEN` | 空 | 设置后开放 `POST /mcp` |
