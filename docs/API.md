# API Contract

所有 `/api/*` 路由除登录与健康检查外都需要 `rss_session` Cookie。

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

- `GET /api/v2/brief/now?hours=12|24`：返回半日/全天全面报告、主题分节、候选/入选统计和事件级五维评分。省略 `minutes` 时 `mode=report`，12/24 小时分别最多入选 30/48 个通过证据门槛的去重事件；旧客户端仍可传 `minutes=3|10|20` 使用有限简报模式。
- `GET /api/v2/events/{id}`：返回事件卡、`event_revisions` 时间线与按质量排序的原始证据。
- `GET /api/v2/watches`
- `POST /api/v2/watches`：`{ "name": string, "query": string, "min_impact": 0..100, "notification_mode": "off" | "brief" | "urgent", "enabled": boolean }`
- `PATCH /api/v2/watches/{id}`：body 同创建。
- `DELETE /api/v2/watches/{id}`
- `POST /api/v2/feedback`：`{ "event_id": uuid, "kind": "more" | "less" | "known" | "irrelevant" | "bad_cluster" }`

当前 `independent_source_count` 按订阅对应站点的规范化 publisher hostname 去重；同一站点的多个 feed 不再冒充独立来源。它仍只是第一阶段近似，不等价于媒体所有权、通讯社转载或共同原始信源的完整独立性判断。

`GET /api/v2/wire/now` 中的 `cited_source_count` 只统计核心快讯句 `claim_id=summary` 的已核验来源，不统计 bullets 或 disagreement 的旁支引用。

## OSINT

- `GET /api/v2/osint/tools`：会话鉴权，返回 `native-osint-v2`、`network_access=false`、`model_required=false` 与本地工具/预算。
- `POST /api/v2/events/{id}/osint/research`：`{ "query"?: string }`。站内检索词 1–240 字符，默认事件标题（最多 240 字符）；输入作为字面词项，不执行搜索字段/操作符。只读本地 PostgreSQL 和 Tantivy，不发 HTTP 或模型请求。返回 `rss-osint-research-v2`：工具步骤、相关材料线索、最多 30 篇事件材料与正文指纹、来源时间、规则片段和待核对发现。`status` 为 `analyzed / insufficient`，不判定真伪。正文每篇最多 12,000 字符，`text_sha256` 对应导出文本，`full_text_sha256` 对应规范化完整正文，完整指纹用于同源合并。响应 `no-store`，并发/同事件锁返回 429，空或过长检索词返回 400。v1 的 `provider` / `analyze` 字段现返回 422，不会悄悄调用旧接口。见 [本地调查 Agent](OSINT_RESEARCH.md)。

- `GET /api/v2/events/{id}/osint`：只读本地材料。返回最多 60 篇未静音材料、`total_sources`、已知来源依赖组、快讯引文/分歧、分析快照时间和证据缺口；`reported_published_at` 保留 `null`，不以 `fetched_at` 冒充发布时间。分组是同发布者、同规范链接、同非空正文指纹关系的传递闭包，不是现实独立性保证。`core_support_groups` 只数已完成快讯中 `verified=true` 且 `claim_id=summary` 的材料所属组。
- `POST /api/v2/events/{id}/evidence/{article_id}/capture`：`{ "include_archive"?: false }`。仅抓取已属于该事件且未静音的文章 URL。限制公开 HTTP(S)、禁止 URL 凭据、每次重定向重新校验地址；不使用阅读器 Cookie 或浏览器会话。每篇材料最多每分钟一次，并发请求返回 429。响应体限 1 MiB，超限/中断/超时不保存不完整记录；HTTP 错误页可留存但标为 `http_error`。保存精确响应体字节、SHA-256、最终 URL、HTTP 状态、允许的响应头及页面自述元数据。
- 第三方存档接口已移除；`include_archive=true` 返回 400，且在读取页面前拒绝。新记录的 `archive.status=not_requested`，历史留存中的存档元数据仍可读取和导出。
- `GET /api/v2/osint/captures/{id}/export`：下载 `rss-osint-evidence-v1` JSON，包括 `body_hex`、`sha256`、字节数和取证元数据。要求会话，响应为 attachment、nosniff、no-store；不会渲染留存的 HTML。

取证使用新增 `0013_osint_captures.sql`。记录只追加，随原文章或事件删除而级联清理；长期留存需导出。摘要和取证快照彼此独立，重新抓取不会自动改变已核验主张或发布快讯。详情与方法边界见 [OSINT 方法](AI_SUMMARIZATION.md#osint-调查)。

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
