# API Contract

所有 `/api/*` 路由除登录与健康检查外都需要 `rss_session` Cookie。

## Auth

- `POST /api/auth/login`：`{ "username": string, "password": string }`
- `POST /api/auth/logout`
- `GET /api/auth/me`
- `PATCH /api/auth/credentials`：`{ "username": string, "current_password": string, "new_password"?: string }`

## Reader

- `GET /api/folders`
- `POST /api/folders`：`{ "title": string }`
- `GET /api/feeds`
- `POST /api/feeds`：`{ "feed_url": string, "title"?: string, "folder_id"?: uuid }`
- `POST /api/feeds/{id}/fetch`
- `DELETE /api/feeds/{id}`
- `GET /api/social-sources`：返回 ActivityPub/RSSHub 可用状态和 OpenCLI 白名单目录；不会返回命令参数或 token。
- `POST /api/social-sources`：`{ "adapter": "activitypub" | "rsshub" | "opencli", "path": string, "title"?: string, "folder_id"?: uuid }`。先实抓验证，成功后创建内部 Hub 订阅。
- `GET /api/articles?feed_id=&folder_id=&unread=&favorite=&limit=&offset=`
- `GET /api/articles/{id}`
- `PATCH /api/articles/{id}`：`{ "is_read"?: boolean, "is_favorite"?: boolean }`
- `GET /api/search?q=keyword&limit=50`

## World Radar v2

- `GET /api/v2/brief/now?minutes=3|10|20&hours=12|24|72`：返回有限事件简报、候选/入选统计和事件级五维评分。
- `GET /api/v2/events/{id}`：返回事件卡、`event_revisions` 时间线与按质量排序的原始证据。
- `GET /api/v2/watches`
- `POST /api/v2/watches`：`{ "name": string, "query": string, "min_impact": 0..100, "notification_mode": "off" | "brief" | "urgent", "enabled": boolean }`
- `PATCH /api/v2/watches/{id}`：body 同创建。
- `DELETE /api/v2/watches/{id}`
- `POST /api/v2/feedback`：`{ "event_id": uuid, "kind": "more" | "less" | "known" | "irrelevant" | "bad_cluster" }`

当前 `independent_source_count` 按订阅对应站点的规范化 publisher hostname 去重；同一站点的多个 feed 不再冒充独立来源。它仍只是第一阶段近似，不等价于媒体所有权、通讯社转载或共同原始信源的完整独立性判断。

`GET /api/v2/wire/now` 中的 `cited_source_count` 只统计核心快讯句 `claim_id=summary` 的已核验来源，不统计 bullets 或 disagreement 的旁支引用。

## Summary

- `GET /api/summaries/{article_id}`
- `POST /api/summaries`：`{ "article_id": uuid }`
- `POST /api/summaries/batch`：`{ "folder_id"?: uuid, "feed_id"?: uuid, "unread_only": boolean, "limit"?: number }`

## Translation

- `POST /api/translations`：`{ "article_id": uuid, "target_language"?: "zh-Hans" | "en" }`
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
