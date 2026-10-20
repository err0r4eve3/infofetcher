# API Contract

所有 `/api/*` 路由除登录与健康检查外都需要 `rss_session` Cookie。`/api/v2/intel/*` 另外接受 `Authorization: Bearer <INTEL_API_TOKEN>`。

## Auth

- `POST /api/auth/login`：`{ "token": string }`
- `POST /api/auth/logout`
- `GET /api/auth/me`

## Reader

- `GET /api/folders`
- `POST /api/folders`：`{ "title": string }`
- `GET /api/feeds`
- `POST /api/feeds`：`{ "feed_url": string, "title"?: string, "folder_id"?: uuid }`
- `POST /api/feeds/{id}/fetch`
- `DELETE /api/feeds/{id}`
- `GET /api/social-sources`：返回 ActivityPub/RSSHub 可用状态和 OpenCLI 白名单目录；不会返回命令参数或 token。
- `POST /api/social-sources`：`{ "adapter": "activitypub" | "rsshub" | "opencli", "path": string, "title"?: string, "folder_id"?: uuid, "source_type"?: "adapter", "language"?: string, "region"?: string, "quality_score"?: number }`。先实抓验证，成功后创建内部 Hub 订阅。推荐 RSSHub 来源可附带分类元数据；未提供时继续按社交来源和质量分 58 创建。
- `GET /api/articles?feed_id=&folder_id=&unread=&favorite=&limit=&offset=`
- `GET /api/articles/{id}`
- `PATCH /api/articles/{id}`：`{ "is_read"?: boolean, "is_favorite"?: boolean }`
- `GET /api/search?q=keyword&limit=50`

## World Radar v2

- `GET /api/v2/brief/now?hours=24|12`：返回全天/半日全面报告（省略 `hours` 时 24 小时）、主题分节、候选/入选统计和事件级五维评分。省略 `minutes` 时 `mode=report`，24/12 小时分别最多入选 90/60 个通过证据门槛的去重事件；旧客户端仍可传 `minutes=3|10|20` 使用有限简报模式。
- `GET /api/v2/events/{id}`：返回事件卡、`event_revisions` 时间线与按质量排序的原始证据。
  - `evidence` 保留兼容排序字段 `published_at`，新增真实可空的 `reported_published_at` 与 `fetched_at`。`provenance` 分开返回文章链接站点 `link_host`、订阅入口 `feed_host`、已知聚合链接标记及 `duplicate_group`；站点不等于已核验的原始发布者。重复组仅连接相同规范链接或非空正文指纹的材料，不按媒体名称合并，不识别转载方向，未匹配不代表独立。
  - `collection` 返回对应订阅源的最近成功时间和连续失败次数。`evidence_scope.total_articles` 为该事件全部未静音材料数；其余分组及采集状态统计仅覆盖质量排序前 30 条返回材料，不代表完整媒体样本，也不推断报道盲区。无新增采集或外部调用。
- `GET /api/v2/watches`
- `POST /api/v2/watches`：`{ "name": string, "query": string, "min_impact": 0..100, "notification_mode": "off" | "brief" | "urgent", "enabled": boolean }`
- `PATCH /api/v2/watches/{id}`：body 同创建。
- `DELETE /api/v2/watches/{id}`
- `POST /api/v2/feedback`：`{ "event_id": uuid, "kind": "more" | "less" | "known" | "irrelevant" | "bad_cluster" }`

当前 `independent_source_count` 按订阅对应站点的规范化 publisher hostname 去重；同一站点的多个 feed 不再冒充独立来源。它仍只是第一阶段近似，不等价于媒体所有权、通讯社转载或共同原始信源的完整独立性判断。

`GET /api/v2/wire/now` 中的 `cited_source_count` 只统计核心快讯句 `claim_id=summary` 的已核验来源，不统计 bullets 或 disagreement 的旁支引用。

## OSINT

- `GET /api/v2/osint/tools`：会话鉴权，返回 `native-osint-v4`、`network_access=true`、`default_mode=online`、`model_required=false`、实际搜索提供方与工具预算。
- `POST /api/v2/events/{id}/osint/research`：`{ "query"?: string, "mode"?: "online" | "local" }`。检索词 1–240 字符，默认事件标题（最多 240 字符）；输入作为字面词项，不执行搜索字段/操作符。默认在本地 PostgreSQL/Tantivy 材料上增加有界联网检索与正文读取；local 模式不发 HTTP，两种模式均不调用模型。结果事务保存到调查档案。返回 `rss-osint-research-v4`：研究状态/停止原因、每轮检索和候选读取记录、稳定证据 ID/字节定位/引用校验、工具步骤、相关材料线索、最多 30 篇事件材料、20 份本地文件及 6 篇站外正文与指纹、来源时间、规则片段和待核对发现。`status` 为 `analyzed / insufficient`，不判定真伪。正文每篇最多 12,000 字符，`text_sha256` 对应导出文本，`full_text_sha256` 对应规范化完整正文，完整指纹用于同源合并。响应 `no-store`，并发/同事件锁返回 429，空或过长检索词返回 400。v1 的 `provider` / `analyze` 字段现返回 422，不会悄悄调用旧接口。见 [调查 Agent](OSINT_RESEARCH.md)。

- `GET /api/v2/events/{id}/osint`：只读本地材料。返回最多 60 篇未静音材料、`total_sources`、已知来源依赖组、快讯引文/分歧、分析快照时间和证据缺口；`reported_published_at` 保留 `null`，不以 `fetched_at` 冒充发布时间。分组是同发布者、同规范链接、同非空正文指纹关系的传递闭包，不是现实独立性保证。`core_support_groups` 只数已完成快讯中 `verified=true` 且 `claim_id=summary` 的材料所属组。
- `POST /api/v2/events/{id}/evidence/{article_id}/capture`：`{ "include_archive"?: false }`。仅抓取已属于该事件且未静音的文章 URL。限制公开 HTTP(S)、禁止 URL 凭据、每次重定向重新校验地址；不使用阅读器 Cookie 或浏览器会话。每篇材料最多每分钟一次，并发请求返回 429。响应体限 1 MiB，超限/中断/超时不保存不完整记录；HTTP 错误页可留存但标为 `http_error`。保存精确响应体字节、SHA-256、最终 URL、HTTP 状态、允许的响应头及页面自述元数据。
- 第三方存档接口已移除；`include_archive=true` 返回 400，且在读取页面前拒绝。新记录的 `archive.status=not_requested`，历史留存中的存档元数据仍可读取和导出。
- `GET /api/v2/osint/captures/{id}/export`：下载 `rss-osint-evidence-v1` JSON，包括 `body_hex`、`sha256`、字节数和取证元数据。要求会话，响应为 attachment、nosniff、no-store；不会渲染留存的 HTML。

取证始于 `0013_osint_captures.sql`，`0014_osint_casebook.sql` 为旧记录回填档案并移除对原文章/事件的级联删除。记录独立留存，不受阅读器清理影响；数据库容量随留存增长。摘要和取证快照彼此独立，重新抓取不会自动改变已核验主张或发布快讯。详情与方法边界见 [OSINT 方法](AI_SUMMARIZATION.md#osint-调查)。

### 调查档案与人工复核

以下接口均要求会话，响应 `no-store`。列表每页最多 50 条，返回 `next_offset`；查询词最多 240 字符。

- `GET /api/v2/osint/cases?q=&offset=0`：档案标题检索、调查/取证/文件数量、原事件是否仍存在。
- `GET /api/v2/osint/cases/{id}?offset=0`：档案、调查版本和原站取证元数据，二者均按 50 条分页。
- `GET /api/v2/events/{id}/osint/history`：事件已有档案首页，没有档案返回 `null`；原事件清理后仍可读取。
- `GET /api/v2/osint/runs/{id}`：不可变报告、`report_sha256`、追加复核历史、实体标注、关系和操作历史。
- `GET /api/v2/osint/runs/{id}/export`：attachment JSON `infofetcher-case-export-v1`；包含完整调查版本及复核/关联，不内嵌原始响应体。
- `POST /api/v2/osint/runs/{id}/reviews`：`{ target_kind: "finding" | "fragment", target_index: number, verdict: "confirmed" | "rejected" | "needs_review", note: string }`。索引属于该不可变报告，理由 1–2000 字符；不覆盖旧意见。
- `GET /api/v2/osint/entities?q=&offset=0`：按名称/别名检索实体。
- `POST /api/v2/osint/runs/{id}/entities`：新建时 `{ kind: "person" | "organization" | "location", name, aliases?: string[], article_id, excerpt }`；关联已有实体时仅 `{ entity_id, article_id, excerpt }`。名称/别名 2–120 字符，最多 12 个额外别名；片段 2–1000 字符且必须在该版本对应正文中逐字存在。名称或别名必须命中片段，不自动合并同名。
- `GET /api/v2/osint/entities/{id}?offset=0`：实体及跨档案标注，区分 suggested / confirmed / rejected。
- `POST /api/v2/osint/entities/{id}/scan`：`{ before?: uuid | null }`，按 `(created_at,id)` 游标每批检查 20 个保存版本，返回 `scanned_runs / added / next_before`；仅创建 suggested 标注，不覆盖已有人工判断。达到并发预算返回 429。
- `POST /api/v2/osint/mentions/{id}/review`：`{ status: "confirmed" | "rejected", note }`，保存人工结论与操作历史。
- `POST /api/v2/osint/runs/{id}/relations`：`{ subject_id, object_id, predicate, article_id, excerpt }`，两端须已在本版人工确认且不同，关系名 1–80 字符，引文须存在；初始状态 confirmed 表示人工记录，不是算法证明。
- `POST /api/v2/osint/relations/{id}/review`：`{ status: "confirmed" | "rejected", note }`，保留前后状态和理由。

## 编辑层

设计与配置见 [EDITORIAL_LAYER.md](EDITORIAL_LAYER.md)。

- `GET /api/v2/brief/now` 与 `GET /api/v2/events/{id}` 的事件新增：`editorial_profile`、`editorial_score`（0–10，未评审为 `null`）、`editorial_tags`、`history_note` 与 `history_refs[]`（`event_id`、`title`、`digest_date`，日期是本站记录日期，不是事件日期）、`community_note`（评论者观点）、`takeaway`（仅 `long-read`）。有评审时 `impact_score = editorial_score × 10`，`why_it_matters` 为评审理由；低于 Profile 阈值的事件不进简报（命中关注除外）。`stats.assessed_events` 为已评审的候选数。
- `GET /api/v2/editorial/status`：评审开关、24 小时评审数与失败数、各 Profile 阈值、最近错误。
- `GET /api/feeds` 新增 `editorial_profile` 与 `quality_stats`（`snr`、`average_score`、`passed_events`、`assessed_events`、`auto_adjust`、`declining`）。`PATCH /api/feeds/{id}` 接受 `editorial_profile`：`world-news` | `tech-research` | `long-read` | `auto`。
- `GET /api/v2/brief/editions`：最近 60 期日报与订阅路径；`POST` 立即生成（或重新生成）今天的一期，不推送。
- `GET /api/v2/brief/editions/{YYYY-MM-DD}`：一期日报 JSON；`?format=markdown` 返回 Markdown。
- `GET /api/v2/brief/editions/{YYYY-MM-DD}/card.png`：一图流日报 PNG（需登录）。订阅器用 `GET /feeds/brief/{YYYY-MM-DD}.png?token=…`（同 `BRIEF_FEED_TOKEN`）。
- `GET /feeds/brief.xml?token=…`：最近 30 期的 Atom 订阅。需配置 `BRIEF_FEED_TOKEN`，未配置时 404。
- `POST /mcp`：MCP Streamable HTTP（JSON 响应，无 SSE），`Authorization: Bearer <MCP_API_TOKEN>`。工具：`brief_now`、`wire_now`、`search_articles`、`event_detail`、`brief_edition`。未配置 token 时 404。

## Intel（交易 Agent 只读）

时间点查询：`as_of` 必填（RFC 3339，不能晚于当前时间 2 分钟以上），只返回 `known_at <= as_of` 的文章。`known_at` 是首次入库时间，不是源站发布时间。

- `GET /api/v2/intel/news?asset=BTC&as_of=&lookback_hours=24&limit=20`：资产相关文章，每个事件一篇。`asset` 支持 BTC、ETH、SOL、XRP、BNB、DOGE、CRYPTO，接受 `BTCUSDT`、`BTC-USD` 等写法。标题命中或正文前 4000 字命中 ≥3 次才算相关，标题命中优先。
- `GET /api/v2/intel/global?as_of=&lookback_hours=24&limit=20`：质量分 ≥60 的全局事件，按 `as_of` 时已入库的佐证来源数排序。
- 参数范围：`lookback_hours` 1–336，`limit` 1–50。
- 返回字段：`as_of`、`window_start`、`asset`、`known_at_semantics`、`items[]`（`article_id`、`story_id`、`title`、`url`、`source`、`source_type`、`language`、`quality_score`、`published_at`、`known_at`、`corroborating_sources`、`excerpt`）。
- 局限：事件分组使用当前聚类结果，不是时间点快照；佐证来源数只统计 `as_of` 前入库的文章。

## Summary

- `GET /api/summaries/{article_id}`
- `POST /api/summaries`：`{ "article_id": uuid }`
- `POST /api/summaries/batch`：`{ "folder_id"?: uuid, "feed_id"?: uuid, "unread_only": boolean, "limit"?: number }`

## Translation

- `POST /api/translations`：`{ "article_id": uuid, "target_language"?: "zh-Hans" | "en" }`
- `POST /api/translations/selection`：`{ "text": string, "source_language"?: string, "target_language"?: "zh-Hans" | "en" }`；单次最多 4,000 字符，响应为 `{ "translation", "target_language", "model" }`，不持久化选区或译文。
- `GET /api/articles/{id}` 的 `translation` 字段只返回与当前正文 hash 匹配的译文；正文变化后旧译文不会继续展示。
- 翻译结果包含 `status / model / source_hash / prompt_version / translated_title / translated_content / error / updated_at`，原文内容不会被覆盖。

摘要响应保留原有 `summary / bullets / keywords / confidence / citations` 字段，并新增：

- `method`：`grounded-article-v3`、`grounded-event-v3`、`extractive-local` 等。
- `scope`：`article` 或 `event_snapshot`。
- `source_count`、`source_article_ids`、`input_hash`：本次来源快照；`cited_source_count` 是最终实际被引用的来源数。生成式路径中的引用均通过 verifier；本地抽取则按原文归属计数。
- `grounding_score`：0-100 的材料支撑度，不是外部事实真伪概率；本地抽取未运行 verifier 时为 `null`。
- `corroboration`：`single / multi / divergent`。
- `disagreements`：通过证据核验的来源分歧。
- `citations[]`：新记录包含 `claim_id / evidence_id / article_id / source_title / article_title / url / excerpt / verified`；最终 `claim_id` 使用 `summary`、`bullet:{index}`、`disagreement:{index}`，可直接对应展示位置。生成式摘要只有通过第二阶段 verifier 的片段才是 `verified=true`；本地原文抽取与旧字符串引用均为 `false`，由 `method` 区分。旧字符串引用仍兼容读取。

## Keyword Detection

- `GET /api/keyword-rules`
- `POST /api/keyword-rules`：`{ "name": string, "keywords": string[], "match_title": boolean, "match_content": boolean, "case_sensitive": boolean, "auto_summary": boolean, "telegram_enabled": boolean, "enabled": boolean, "feed_ids": uuid[], "folder_ids": uuid[] }`
- `PATCH /api/keyword-rules/{id}`：body 同创建。
- `DELETE /api/keyword-rules/{id}`
- `POST /api/keyword-rules/{id}/scan`：`{ "limit"?: number }`
- `GET /api/keyword-matches?rule_id=&limit=&offset=`
- `POST /api/keyword-matches/actions`：`{ "match_ids": uuid[], "summarize": boolean, "telegram": boolean }`

## OPML

- `GET /api/opml/export`
- `POST /api/opml/import`：request body 为 OPML XML。

## Hub

- `GET /hub/echo`
- `GET /hub/github-releases/{owner}/{repo}`
- `GET /hub/hn-top`
- `GET /hub/opencli/{name}`：运行 `OPENCLI_FEEDS_JSON` 白名单中的 OpenCLI 命令并返回 RSS。
- `GET /hub/activitypub/{host}/{username}`：读取公开 ActivityPub outbox。
- `GET /hub/rsshub/{route}`：通过配置的 `RSSHUB_BASE_URL` 代理 RSSHub route。
- `GET /hub/web-watch/{name}`：抓取 `WEB_WATCHES_JSON` 白名单中的页面选择器。

由 `POST /api/social-sources` 创建的来源会写入 `feeds.hub_adapter/hub_path`，worker 在进程内执行 adapter；不会把 OpenCLI 访问 token 持久化到 `feed_url`。

### 本地文件与持久任务

以下接口均需会话并返回 `no-store`，不调用外部识别或模型服务。

- `POST /api/v2/events/{id}/osint/documents?filename=...` 或 `POST /api/v2/osint/cases/{id}/documents?filename=...`：请求体为文件原始字节，建议 `Content-Type: application/octet-stream`。返回 202 `{id, case_id, job_id, duplicate}`。事件入口可自动建立档案。支持 PDF/PNG/JPEG/UTF-8 文本，10 MiB/文件、20 文件/档案；类型按字节检查。相同档案内同 SHA-256 返回旧文件。
- `GET /api/v2/osint/cases/{id}/documents`：文件元数据、状态、字节数、哈希、任务 ID 和错误，不返回原件或全文。
- `GET /api/v2/osint/documents/{id}`：原件 SHA-256、提取记录 SHA-256、逐页文本、方法、页码与截断标记；未成功提取时 `extraction=null`。
- `GET /api/v2/osint/documents/{id}/original`：原件下载，attachment / octet-stream / nosniff / CSP sandbox。
- `POST /api/v2/events/{id}/osint/jobs` 或 `POST /api/v2/osint/cases/{id}/jobs`：`{query?: string, mode?: "online" | "local"}`。新请求默认 online。冻结本次本地分析输入后返回 202 `{id,case_id,existing}`；同档案已有 queued/running 调查时返回其 ID，不新增任务。档案入口可在原事件清理后继续分析文件。
- `POST /api/v2/osint/topics`：主动搜索尚未聚类的事件。`{query: string, mode?: "online" | "local"}`，query 为 1–240 字符。按 query（忽略大小写）复用或新建 `origin="topic"` 档案，再按上一条规则排队，返回 202 `{id,case_id,existing}`。主题档案没有事件素材：站内全文检索命中（最多 30 篇，排除静音来源）作为初始材料，online 模式再检索公开网页。档案对象新增 `origin: "event" | "topic"`；主题档案的 `event_id` 是合成键，`event_available` 恒为 false。
- `GET /api/v2/osint/cases/{id}/jobs`：优先列出活动任务，再列最新历史，最多 50 条。包含 kind/status/attempts/error/run_id/lease_until，不泄露租约 token 或冻结输入。
- `POST /api/v2/osint/jobs/{id}/cancel`：使 queued/running 任务取消，已结束任务保持原状态，返回 `{ok:true}`。
- `POST /api/v2/osint/jobs/{id}/retry`：仅允许 failed/cancelled，保留原输入、清除错误、重新排队。已完成/活动任务返回 400，同档案已有活动调查返回 429。

任务状态：queued/running/done/failed/cancelled；文件状态：queued/running/ready/failed/cancelled。报告新增 `input_captured_at`，文件材料新增 `document`（文件 ID、原件哈希、提取哈希、逐页方法和哈希）；旧报告字段保持兼容。文件 UUID 作为材料 ID，可用于原有实体/片段/关系接口。历史同步调查接口仍保留，页面改用队列。
