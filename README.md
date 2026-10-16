# Infofetcher

自托管、事件与变化驱动的世界信息入口。RSS、ActivityPub、RSSHub 和 OpenCLI 是采集层；用户面对的是有限简报、事件 Ledger、Watch 和可回溯证据。后端使用 Rust `axum`、Postgres、Tantivy 和持久化 worker；前端使用 React/Vite/TypeScript。

## 功能

- 单用户本地管理员登录，Cookie session 保护 API 和 UI。
- 默认首页生成过去 12 小时或 24 小时的全面报告，按全球、AI/科技、安全、商业和其他重要变化分节；连续快讯保留在独立页面。
- 事件 Ledger 持久记录 `ADD / UPDATE / CORRECT / CONTRADICT / CONFIRM`，事件详情展示时间线与来源证据。
- Watch 统一主题、实体和问题关注；“多看/少看/已知/聚类有误”反馈直接参与后续排序。
- 原三栏阅读器保留为“全部资料”，继续提供质量/时间排序、来源偏好和连续加载。
- 同一事件聚类与多来源核对；摘要采用证据 ID、原子主张、二次蕴含校验和可点击原文引用。
- RSS、Atom、JSON Feed、HTML 自动发现，支持 HTTP/2 协商、ETag/Last-Modified、WebSub、失败退避和持久化队列；源站/解析失败不做短时间热重试，持续失败最长退避 24 小时，数据库或索引等内部故障仍由 worker 有界重试。
- 内置 `/hub/:adapter/*path` 信息适配器：GitHub releases、Hacker News、ActivityPub、RSSHub 代理、白名单网页变化监控和 OpenCLI。
- 文章正文抽取，feed 内容不足时抓取原文 HTML 并做 Readability-style 正文提取。
- 原文站点需要登录态时，可通过 `ARTICLE_FETCH_COOKIES_JSON` 为指定 hostname 配置 Cookie header。
- Tantivy 嵌入式全文搜索，可从 Postgres 重建索引。
- OpenAI 兼容 API 摘要任务，支持单篇/事件快照、未读批量总结、严格结构化输出和本地抽取降级。
- 按需文章翻译，使用独立的 OpenAI-compatible 配置、非思考模式和持久化译文，不覆盖原文。
- 关键词检测规则，支持按订阅/分类选择目标、记录命中，并批量触发 AI 总结或 Telegram 推送。
- OPML 导入/导出。
- OSINT 调查 Agent（默认联网，可选本地）：持久档案与版本对比、本地 PDF/图片 OCR 与原件留存、可恢复后台任务、人工复核历史、人物/机构/地点标注、跨事件别名候选与有依据的关系记录，保留站内检索、转载/数字/更正线索及 JSON 导出，支持缺口补查、正文证据 ID 与引用校验；默认 GDELT 新闻检索，可用 `OSINT_SEARXNG_URL` 接入通用网页检索，无需模型接口，见 [实现与边界](docs/OSINT_RESEARCH.md)。

## 本地开发

```bash
cp .env.example .env
docker compose up -d postgres
cargo run -p infofetcher-api
npm install
npm run dev:web
```

登录只需一个 token。启动前在未跟踪的 `.env` 中配置随机 `AUTH_TOKEN`（32–256 个无空白 ASCII 字符，建议 32 字节随机值的十六进制编码），没有默认值。

开发前端默认在 `http://localhost:5173`，API 默认在 `http://localhost:8080`。

## 生产/自托管

```bash
cp .env.example .env
docker compose up --build
```

打开 `http://localhost:8080`。必须配置 `AUTH_TOKEN`；缺失或格式不合法时服务拒绝启动。旧用户名/密码不再用于登录，`ADMIN_USERNAME` 仅保留内部数据归属。token 只在登录时提交，后续请求使用 HttpOnly 会话 cookie。更换 token 后重启 API；若要立即撤销已登录设备，还需清空 `sessions` 表。

默认 `CONTENT_RETENTION_DAYS=365`。服务每 6 小时分批清理超过 365 天且未收藏的文章，以及关联摘要、翻译、关键词命中、事件材料和已完成任务，再同步删除全文索引文档。订阅、账号、Watch、反馈偏好和显式收藏不随内容轮换删除；也可以手动运行 `infofetcher-api prune-retention`。

## 信息质量与排序

- 全面报告从最近 12 或 24 小时内最多 2,000 个候选中先做证据门槛，再选取最多 30 或 48 个去重事件并按主题分节，不用低信号内容填满条数。
- 事件排序分别计算影响、变化新颖度、连续时效衰减、可信度和用户相关性；最终以 MMR 风格重排抑制重复标题、正文、URL、主题和 publisher 集中。
- Watch 命中和跨事件反馈参与用户相关性，但不能绕过证据门槛；单一来源事件会明确标记“等待交叉确认”。
- 设置页仍可提高/降低来源权重或静音来源；旧主题监控会在迁移时导入 Watch。
- 编辑层（`EDITORIAL_ENABLED=true`）用 LLM 按内容类型（时事要闻、科技与研究、深度长文）给候选事件打 0–10 分，替代关键词影响分，并补充“此前进展”和社区观点；同时计算信源信噪比、按栏目限额，每天生成日报（Atom、Webhook、Telegram），并提供 MCP 接口。见 [`docs/EDITORIAL_LAYER.md`](docs/EDITORIAL_LAYER.md)。
- `rebuild-intelligence` 会批量重算全部文章质量、语言和事件聚类；`recluster-recent` 默认只重算最近 36 小时、来源质量不低于 60 的文章，适合修复线上近期漏合并；`reindex` 会分批清理并重建全文索引。
- `migrate` 只应用数据库迁移后退出，适合在生产容器切换前单独执行迁移。

选择算法、生产基线、反馈语义和后续评估方案见 [`docs/INFORMATION_FEED_SELECTION.md`](docs/INFORMATION_FEED_SELECTION.md)。

## 证据约束摘要

- 短内容或未配置 `LLM_API_KEY` 时直接抽取原文，明确标记为本地抽取，不伪装成生成式核验。
- 普通文章会从全文而非只从开头选择可寻址证据；同事件摘要最多综合 5 个不同 hostname、feed 和正文 hash 的来源。
- 模型先生成绑定 evidence ID 的原子主张，再由第二次窄调用逐条判断 `entailed / contradicted / insufficient`。
- 数字、年份、证据 ID 和输出长度还会经过本地校验；引用 URL、标题和 excerpt 只由服务端生成。
- UI 中的“材料支撑度”只表示摘要是否受输入材料支持，不表示现实世界事实真伪。
- provider 不支持 JSON Schema 时会降级到 JSON Object / prompt JSON，并在当前进程内按 provider/model 记住探测结果，避免每个阶段重复发送必然失败的能力探测；所有模式都执行同一严格 decoder，不再接受自由文本摘要。
- `LLM_MAX_CONCURRENCY` 默认 2，独立限制摘要正文与两阶段模型调用的并发，避免批量摘要挤占抓取资源。
- Feed 抓取与摘要使用独立执行池；多实例连接同一数据库时，以 PostgreSQL advisory lock 选出唯一 worker leader。每次 claim 另带数据库 `run_token` 做完成/失败 CAS，失去领导权时会取消本任期任务，避免旧实例覆盖新结果。
- `LLM_MAX_OUTPUT_TOKENS` 默认 4096；OpenAI 兼容 provider 使用 `max_completion_tokens`，官方 DeepSeek API 使用 `max_tokens` 并显式关闭思考模式。文章摘要输出合同限制为最多 1 条核心、4 条要点和 2 组双位置分歧。事件快讯只请求 1 条补充要点和 1 组分歧，输出上限为 8192，标题直译上限为 2048；更低的全局配置仍优先。默认开启思考的模型（如 CPA 上的 claude-haiku-5-5）会先消耗约 2k 隐藏推理 token，上限过低会截断输出并让整次调用失败；思考 token 无论如何都计费，提高上限只挽回原本失败的调用。旧 provider 不支持输出上限时可设为 `0` 省略。对已知 reasoning model 会省略不兼容的 `temperature`。
- `LLM_CONTEXT_BUDGET_CHARS` 默认 64000；服务端预留约 20000 字符给 system prompt、JSON Schema、来源目录和输出，再把其余预算均分给证据正文。事件快讯的总上下文额外限制为 48000，普通文章保持完整配置。provider 支持原生 JSON Schema 时，Schema 只随 `response_format` 发送一次；兼容降级时才补入 prompt。
- `EVENT_AGENT_UPDATE_DEBOUNCE_SECONDS` 默认 1800。同一事件首次出现时立即生成；后续普通来源更新最多每 30 分钟重算一次，`CORRECT` / `CONTRADICT` 修订不等待该窗口。
- 成功响应会把 provider 返回的 `prompt_tokens / completion_tokens / total_tokens`、思考 token、prompt cache 命中/未命中 token 与本地输入字符数写入日志，便于持续核对优化是否生效；日志不记录正文或 API Key。
- 核验请求将主张全文按 `claim_id` 只发送一次，各条证据检查引用同一主张，减少多来源核验中的重复输入；证据全文、逐条核验结果和输出校验保持不变。实际 token 节省以 provider usage 为准。

方法选择、研究依据、质量门槛与已知边界见 [`docs/AI_SUMMARIZATION.md`](docs/AI_SUMMARIZATION.md)。

## 按需翻译

阅读器工具栏中的语言按钮会按需翻译全文并单独保存；原文始终保留，正文更新后旧译文自动失效。在原文正文中选中文字，还会出现贴近选区的“译成中文/英文”操作；划词结果只保留在当前页面，不写入文章译文表，同一选区会在页面内复用结果，避免重复调用。

翻译服务与摘要服务使用两组独立配置，默认兼容 OpenAI API：

```bash
TRANSLATION_BASE_URL=https://cpa.ladymiao.net/v1
TRANSLATION_API_KEY=
TRANSLATION_MODEL=claude-haiku-5-5
TRANSLATION_TIMEOUT_SECONDS=60
TRANSLATION_MAX_CONCURRENCY=2
TRANSLATION_MAX_INPUT_CHARS=60000
TRANSLATION_CHUNK_CHARS=6000
TRANSLATION_MAX_OUTPUT_TOKENS=8192
```

文章正文和划词内容都按不可信数据处理，模型必须翻译其中的祈使句而不能执行它们；响应必须通过严格 JSON 解码后才会返回或持久化。划词翻译单次最多 4,000 字符，并与全文翻译共享并发限制。失败会明确返回错误，不会用低质量本地译文伪装成正式结果。

事件快讯标题优先使用上述独立翻译配置；未配置 `TRANSLATION_API_KEY` 时会改用事件摘要的 `LLM_*` provider。独立翻译遇到不可重试的 4xx 会立即终止重试；若同时配置了主摘要模型，还会在当前进程内熔断独立翻译并切回主模型，避免余额不足等错误反复放大调用。启用 `EVENT_AGENT_ENABLED=true` 时，两组 API Key 至少要配置一组，否则服务会在启动时明确拒绝该无效配置。

## 登录态正文抓取

`ARTICLE_FETCH_COOKIES_JSON` 是 hostname 到 Cookie header 的 JSON 对象。服务端只会把 Cookie 发送给完全匹配的文章 URL hostname。

示例：

```bash
ARTICLE_FETCH_COOKIES_JSON='{"www.nodeseek.com":"session=...; cf_clearance=..."}'
```

## 浏览器正文抓取

需要 JS 渲染或登录态页面时，可以启用本机 Chrome/Chromium 回退抓取。服务端会用 `--dump-dom` 获取渲染后的 HTML，再走现有正文提取；默认关闭。

示例：

```bash
BROWSER_FETCH_ENABLED=true
BROWSER_FETCH_BIN=chromium
BROWSER_FETCH_USER_DATA_DIR=/var/lib/infofetcher/chrome-profile
BROWSER_FETCH_TIMEOUT_SECONDS=45
BROWSER_FETCH_WAIT_MS=5000
```

`BROWSER_FETCH_USER_DATA_DIR` 应使用专用 profile；先在服务器上用同一 profile 完成合法订阅账号登录。不要安装或依赖绕过访问控制的扩展。

## OpenCLI 社媒订阅

设置页的“社媒抓取”支持把白名单 OpenCLI 命令转成内部订阅。服务端会先实际执行一次并确认有条目，再写入 `hub_adapter/hub_path`；后台 worker 直接调用 adapter，不会把 session 或 `OPENCLI_ACCESS_TOKEN` 存进订阅 URL。

先在运行 API 的机器安装 OpenCLI，再用 `OPENCLI_FEEDS_JSON` 暴露固定命令。建议固定已验证版本：

```bash
npm install -g @jackwener/opencli@1.8.6
opencli --version
opencli doctor
```

无需登录的 Bluesky 示例：

```bash
OPENCLI_BIN=opencli
OPENCLI_FEEDS_JSON='{"bluesky-bsky-app":{"title":"Bluesky · bsky.app","link":"https://bsky.app/profile/bsky.app","args":["bluesky","user","bsky.app","--limit","20"]}}'
```

重启 API 后，在设置页选择该白名单来源并点击“测试并添加”。也可只验证 RSS 输出：

```text
http://localhost:8080/hub/opencli/bluesky-bsky-app
```

X/Twitter、小红书、微博等来源会复用 OpenCLI Browser Bridge 所连接的 Chrome 登录态；必须先合法登录目标站点。不要把任意参数透传到公网路由，只在 `OPENCLI_FEEDS_JSON` 中配置明确需要的只读 feed。

公网直接暴露 `/hub/opencli/*` 时，应配置 `OPENCLI_ACCESS_TOKEN`；未提供 token 时仅允许已登录会话。应用内部订阅不需要该 token，并受同一 OpenCLI 并发限制。通用 API 镜像不捆绑 OpenCLI/Chrome；`Dockerfile.homeserver` 固定内置 OpenCLI 客户端，Browser Bridge 和登录 profile 仍由受控宿主机显式提供。

### 服务器端登录抓取

`Dockerfile.homeserver` 固定内置 OpenCLI 1.8.6。生产 API 使用 host network 时，OpenCLI 命令会连接同一服务器 `127.0.0.1:19825` 上的 Browser Bridge；Chromium 登录 profile 和 Cookie 只保留在服务器，不写入仓库或订阅 URL。

宿主机服务模板位于 `deploy/systemd/`：`rss-opencli-daemon.service`、`rss-x-display.service` 和 `rss-x-browser.service`。它们使用专用 `rss-x` 用户、`/opt/rss-x-runtime/chrome-profile` 持久 profile、仅回环监听的调试端口 `19222`，并把 OpenCLI 扩展限定为唯一加载的浏览器扩展。首次登录应通过 SSH 隧道连接临时 VNC，完成后关闭 VNC；不要直接开放 VNC、调试端口或 Bridge 端口。

建议把 X 账号合并成少量搜索流，Bilibili 按创作者配置视频流，降低页面请求和限流风险。`deploy/opencli-feeds.json` 是完整白名单的唯一维护位置：

```bash
export OPENCLI_TIMEOUT_SECONDS=120
export OPENCLI_FEEDS_JSON="$(jq -c . deploy/opencli-feeds.json)"
```

上述命令应在启动 Docker Compose 的同一 shell 中执行；不要把 `$(...)` 表达式原样写入 `.env` 文件。

保持 `OPENCLI_MAX_CONCURRENCY=1`，抓取周期使用应用默认值。登录失败、Bridge 断开或 X 限流时，worker 会记录来源错误并按现有退避逻辑重试，不应把浏览器 Cookie 改存为环境变量。

## ActivityPub、RSSHub 与网页监控

```text
/hub/activitypub/mastodon.social/username
/hub/rsshub/github/advisor/data/reviewed/composer
/hub/web-watch/example
```

RSSHub 代理需要设置 `RSSHUB_BASE_URL`。网页监控只允许 `WEB_WATCHES_JSON` 白名单中的 URL，例如：

```bash
WEB_WATCHES_JSON='{"example":{"title":"Example 变化","url":"https://example.com","selector":"main"}}'
```

设置页可直接输入 ActivityPub 账号（如 `@Mastodon@mastodon.social`）或 RSSHub route（如 `bilibili/popular/all/1`）。“测试并添加”成功后同样走内部 Hub worker，不依赖回抓本机 HTTP 地址。

所有用户可控 HTTP 抓取默认拒绝内网、回环、链路本地和保留地址，并限制响应体大小。只有可信内网部署才应显式设置 `ALLOW_PRIVATE_FETCH=true`。

## Telegram 推送

关键词命中可通过 Telegram Bot API 推送。配置 `.env`：

```bash
TELEGRAM_BOT_TOKEN=123456:bot-token
TELEGRAM_CHAT_ID=123456789
```

未配置时，TG 推送动作会把对应命中标记为失败，不影响 AI 总结或文章抓取。

## 运维命令

```bash
cargo run -p infofetcher-api -- fetch-once
cargo run -p infofetcher-api -- reindex
cargo run -p infofetcher-api -- rebuild-intelligence
RECLUSTER_HOURS=36 RECLUSTER_MIN_FEED_QUALITY=60 \
  cargo run -p infofetcher-api -- recluster-recent
RECLUSTER_ARTICLE_ID=<article-uuid> \
  cargo run -p infofetcher-api -- recluster-article
cargo run -p infofetcher-api -- serve
```

## 验证

```bash
cargo test
cargo clippy -- -D warnings
npm run typecheck:web
npm run build:web
```

## 视觉概念

实现参考的界面概念图保存在：

`/Users/error4ever/.codex/generated_images/019dc8cf-ed5e-74e2-b89d-3401b6b25e1f/ig_018e2b6ced85cd010169edcf49b324819a87f0174578214f52.png`
