# OSINT 调查 Agent 与工具选型

调研日期：2026-09-26。面向新闻事件、公开原文与来源核验。

## 选型

| 项目 | 官方能力 | 本项目采用方式 |
| --- | --- | --- |
| [GPT Researcher](https://github.com/assafelovic/gpt-researcher) | 规划问题、抓取、跟踪来源、生成研究报告；Python/API/MCP | 参考分阶段流程，复用现有 Rust 逐条核验器，避免第二套服务和模型配置 |
| [Stanford STORM](https://github.com/stanford-oval/storm) | 多视角提问与带引用长文 | 采用普通检索与更正/澄清检索两个视角，不运行完整长文工作流 |
| [Open Deep Research](https://github.com/langchain-ai/open_deep_research) | 可配置模型、检索工具、MCP 与 LangGraph 工作流 | 长篇专题候选方案；当前事件级调查不增加 LangGraph 环境 |
| [GDELT DOC 2.0](https://blog.gdeltproject.org/gdelt-doc-2-0-api-debuts/) | 新闻 Article List JSON 与时间范围检索 | 已接入官方固定 API，一次查询近一个月，最多 12 条线索 |
| [SearXNG](https://docs.searxng.org/dev/search_api.html) | GET/POST 检索，配置后返回 JSON | 已接入可配置实例；普通和更正检索共两次，交错合并最多 12 条线索 |
| [Bellingcat Auto Archiver](https://github.com/bellingcat/auto-archiver) | 网页、社媒与影音归档，本地或远程存储 | 影音取证扩展候选；当前沿用已有网页字节留存与 Wayback 查询 |
| [Bellingcat Toolkit](https://bellingcat.gitbook.io/toolkit) | 网页、影像、地理及资料整理工具目录 | 保留人工调查入口，辅助自动化无法核验的材料 |

这不是跨项目性能排名。完整研究框架未在本环境做质量或成本基准；取舍依据是当前事件级、可追溯、有限预算需求。本次无全局程序安装、无任意 shell/MCP 执行入口。

## 已实现流程

1. 在事件 OSINT 面板选择“现有材料 / GDELT / SearXNG”，默认仅本地材料。
2. 读取最多 30 篇未静音、有正文的事件文章。选外部检索时，只发送可见检索词，不向检索器发送本地正文、Cookie 或模型凭据。
3. 搜索结果只是线索；最多读取 4 个公开原文。限标准 HTTP(S) 端口，每页 1 MiB、每次操作 20 秒，每次重定向重新解析、校验并固定 IP。不使用内网、浏览器登录态或环境代理。
4. 从原文提取标题、正文及页面声明的 RFC3339 发布时间；未知时间为空。收录时间不冒充发布时间。只支持 HTML/XHTML/plain text，PDF 和影音仍需人工调查。
5. 按发布者、规范 URL、抽取正文指纹做传递分组。本地和外部交替选取最多 5 个组，各取一篇。不同组不保证现实独立性。
6. 勾选“主张核验”后，复用草案、引文定位、逐条支持性与冲突核验流程。调查主题作为不可信数据进入专用提示。无模型、繁忙、请求失败或退回抽取式摘要时只返回证据清单。
7. 展示过程、来源时间、引文、分歧及失败状态。每篇抽取正文最多保留 12,000 字符，JSON 导出的 SHA-256 对应该段保留文本和选取记录中的正文；精确 HTTP 字节留存仍使用原有“抓取证据”。

每进程最多两个调查，同事件有 PostgreSQL 事务锁；整次最长 180 秒。共享模型并发限制，模型上下文最多 40,000 字符，单次输出最多 2,048 tokens。底层结构化输出兼容重试沿用现有核验器，任务数不等于模型请求数。

报告只返回本次请求并保留在页面；离开/刷新页面会丢失，长期保存请导出。不添加订阅、不修改文章或事件快讯、不推送报告。

## 配置与在线边界

GDELT 无需新增 API Key。本环境实测官方端点返回 HTTP 429，因此本次未确认其成功返回结果的线上路径；实现显示 `unavailable`，不会将失败表示为“没有报道”。收录和时间范围有限，长中文标题可改为主体关键词或英文关键词。

SearXNG 基础地址由运维固定配置，程序追加 `/search`：

```dotenv
OSINT_SEARXNG_URL=http://searxng:8080
```

SearXNG 自身的 `settings.yml` 需启用 JSON：

```yaml
search:
  formats:
    - html
    - json
```

只允许该固定 SearXNG 地址指向自托管内网实例；禁止 URL 凭据、查询参数、片段和服务重定向。公共实例常禁用 JSON 并返回 403（见官方文档），不能据此判断没有结果。已配置也不意味着已通过在线健康检查。

主张核验使用已有 `LLM_*` 配置，不创建模型账号或凭据。

## 验证

```sh
cargo test -p rss-hub-reader-api
cargo clippy -p rss-hub-reader-api -- -D warnings
npm run typecheck:web
npm run build:web
```

隔离 PostgreSQL 先执行项目迁移，再设置 `RSS_OSINT_TEST_DATABASE_URL`，运行 `cargo test -p rss-hub-reader-api research_roundtrip_with_postgres -- --ignored`。测试覆盖真实会话/API/SQL、同事件锁、静音过滤、无模型降级和受控 SearXNG JSON 协议，不访问公网或付费模型。

本次本地验证：145 个普通测试通过，隔离数据库集成测试单独通过，Clippy、前端类型检查和构建通过。浏览器以受控 API 数据验证桌面 1280×950 / 手机 390×844 的调查、工具失败、导出及 429 恢复。精简测试 PostgreSQL 缺少扩展，测试建表时仅略过 `pgcrypto` / `pg_trgm` 声明及 trigram 索引，使用内建 UUID 函数；没有改动项目迁移文件，也未验证完整生产迁移。真实 SearXNG 实例和付费模型路径未测试，未部署生产。
