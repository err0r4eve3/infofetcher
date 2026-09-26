# AI 信息总结方法（2026-07）

本项目采用的不是“把全文交给模型写一段摘要”，而是证据优先的事件快照：

```text
正文与同事件来源
  -> 来源/重复内容过滤
  -> 全文分段与覆盖式证据选择
  -> 结构化原子主张
  -> 本地 evidence ID、数字和长度校验
  -> 第二次文本蕴含核验
  -> 服务端生成引用与材料支撑分
  -> 摘要、要点、来源分歧
```

## 调研结论

截至 2026 年 7 月，没有一种模型或单次 prompt 能同时解决长文覆盖、多文档差异、事实一致性和引用完整性。适合 RSS/新闻流的生产实践是混合管线：短材料直接抽取；普通文章使用证据约束生成；同事件材料先做来源去重，再做多源综合；任何生成结果都必须经过结构和证据门禁。

采用该设计的主要依据：

- [Attribute First, then Generate（ACL 2024）](https://aclanthology.org/2024.acl-long.182/)：先选择可定位的来源片段，再生成文本，可提高局部引用准确性并降低人工核验成本。
- [Scaling Multi-Document Event Summarization（NAACL 2025）](https://aclanthology.org/2025.naacl-short.44/)：在大规模多文档摘要中，全文与检索方法总体优于纯层级压缩；最佳方向是保留全局上下文的混合方案。
- [DiverseSumm（NAACL 2024）](https://aclanthology.org/2024.naacl-long.32/)：多源新闻不能只总结共识；模型对来源差异的覆盖仍明显不足，因此需要显式的 divergence 输出合同和均衡输入。
- [FineSurE（ACL 2024）](https://aclanthology.org/2024.acl-long.51/)：摘要评价应拆成 faithfulness、completeness、conciseness，而不是只看 ROUGE 或一个总分。
- [FENICE（ACL Findings 2024）](https://aclanthology.org/2024.findings-acl.841/)：将摘要拆成原子 claim，再与来源证据做蕴含对齐，能提供更细粒度、可解释的事实一致性检查。
- [OpenAI Structured Outputs](https://openai.com/index/introducing-structured-outputs-in-the-api/)：JSON Schema 约束能显著提高输出结构可靠性，但结构正确不等于内容正确，所以本项目仍执行本地语义校验。
- [Anthropic long-context prompting research](https://www.anthropic.com/research/prompting-long-context)：在回答前先提取相关原文引用，有助于长上下文检索；本项目进一步把引用选择变成可校验的 evidence ID。
- [Improving Long-Context Summarization with Multi-Granularity Retrieval Optimization（AAAI 2026）](https://www.microsoft.com/en-us/research/publication/improving-long-context-summarization-with-multi-granularity-retrieval-optimization/)：长文任务适合多粒度检索、分层表示和结果 refinement，而不是简单扩大输入窗口。

## 当前实现

### 1. 内容路由

- 单篇正文不超过 280 字，或只有至多两个有效证据段：直接抽取原文，不调用 LLM。
- 未配置 `LLM_API_KEY`：使用同一确定性抽取路径，明确标记为 `extractive-local`。
- 普通文章：使用 `grounded-article-v3`。
- 同一 story 中存在不同 publisher 标识、feed、`content_hash` 和规范化 URL 的材料时：最多取 5 个来源，使用 `grounded-event-v3`，摘要仍归属于请求文章，不持久依赖不稳定的 `story_id`。publisher 标识与事件快讯共用站点 hostname / 显式 Hub route 规则；相同原文的跟踪参数不会制造额外佐证。
- 自动补充材料排除已静音来源，并按用户调整后的来源质量、文章质量和时间排序。显式请求的文章或事件焦点仍优先保留；两个入口的原有候选上限分别为 24 和 30 篇，最终最多取 5 篇。
- 事件流程版本升为 `event-wire-agent-v10`。部署后，旧版本快讯会在现有回看窗口和队列上限内重新核验，产生正常的模型调用；本次本地验证未调用模型或触发线上任务。

### 2. 长文与多源输入

正文按中英文句末标点和长度拆成约 620 字的证据段，并得到 `S1:E001` 形式的稳定请求内 ID。`LLM_CONTEXT_BUDGET_CHARS` 默认 64,000，先为 system prompt、JSON Schema、来源目录和输出预留约 20,000 字符，再在剩余证据预算内做选择。来源元数据只放在独立 catalog，不在每个 span 重复。超出预算时，选择器同时考虑：

- 开头和结尾保底；
- 与标题的词项/CJK bigram 相关度；
- 与已选证据的距离，用于覆盖正文中部并抑制位置偏差。

多源预算按来源均分，避免一篇长文吞掉全部上下文。

### 3. 两阶段生成和校验

第一次调用只返回严格 JSON：1 条核心原子主张、最多 4 条要点、最多 2 组双位置来源分歧、关键词及 evidence ID；每条主张最多 220 字并保留两个 evidence ID，既支持多源交叉支撑，也使默认 4096 token 输出预算与 verifier 检查数闭合。服务端随后拒绝：

- 不存在的 evidence ID；
- 空主张或过长主张；
- 引用证据中不存在的数字/年份；
- 仅来自一个来源的“来源分歧”。

第二次调用把每个“主张 × 所引证据”拆成独立检查，但证据正文只在 verifier 输入中出现一次，逐项返回 `entailed / contradicted / insufficient`。服务端只保留真正支持该主张的 evidence；分歧组还必须保留至少两个不同来源、分别通过核验的 position。只有至少一条证据通过的主张能进入最终摘要。引用的文章 ID、标题、URL 和 excerpt 全部由服务端按 evidence ID 生成，模型不能提供或改写这些字段。

优先发送 `response_format=json_schema`；若 OpenAI-compatible provider 以 400/415/422 明确指出不支持 `response_format`、`json_schema`、Structured Output 或 JSON mode，才依次退到 JSON Object 和 prompt JSON。鉴权、模型不存在、拒绝、截断、限流和服务端错误不会伪装成成功本地摘要；HTTP 200 但 schema 违约也会立即 fail closed，不重复付费尝试其他模式。三种模式都使用同一个严格 Rust decoder，不再接受自由文本或“取第一行”降级。

### 4. 展示语义

- `grounding_score` 是输入材料对输出主张的支撑度，不是现实世界真伪概率。
- 本地抽取摘要不运行第二阶段 verifier，因此 `grounding_score` 为 `null`、citation 的 `verified=false`；UI 会明确显示“本地原文片段”，不会把它混成已核验结果或旧版引用。
- `corroboration` 分为 `single / multi / divergent`；`source_count` 表示分析输入数，`cited_source_count` 表示最终实际支撑摘要的来源数，UI 不会把二者混为一谈。
- `confidence` 仅保留用于旧 API 兼容；新 UI 以“材料支撑度”展示，并明确来源数量和分歧。
- `event_snapshot` 保存实际使用的 `source_article_ids` 与 `input_hash`。任一来源正文更新后，相关已完成摘要会被标记失效，要求重新生成。
- `input_hash` 同时包含 pipeline revision、provider、model、上下文预算和输出预算，影响结果的配置变化不会继续复用旧快照。
- 文章摘要的快照标识还包含 feed、publisher 标识、来源名、文章标题和 URL；这些引用信息在生成期间变化时，同样拒绝写入旧快照。报告只有在摘要非空且存在 `grounding_score` 时才增加已核验摘要的排序奖励，本地摘录仍可显示但不获得该奖励。

## 质量门槛

自动化门槛：

- 严格 schema 解析率：100%；
- 最终 citation ID 可解析率：100%；
- 最终 claim 至少有一个合法 evidence ID：100%；
- 数字、正负号、百分比、单位和标识符必须以同型事实 token 出现在所引材料：100%；
- 引用元数据只能由服务端派生：100%；
- 短文和无 key 路径外部调用数：0；
- 旧字符串 citation 与新结构化 citation 都能读取。

离线评测不使用单一总分，应分别记录：主张支撑率、可引用主张覆盖率、引用精确率、要点覆盖、分歧覆盖、冗余率、P50/P95 延迟、输入/输出 token 和每篇成本。

## 事件快讯 Agent

`event_agent.rs` 在文章 worker 之外运行独立的事件驱动循环。它只自动选择最近、多来源且达到质量门槛的事件，并按以下显式阶段执行：

```text
read_event
  -> select_evidence
  -> draft_claims
  -> verify_claims
  -> publish_digest / complete
```

每次运行都有数据库租约、`run_token`、阶段检查点、失败退避和明确的 `publish_digest` 完成信号。Agent 读取的证据、发布到 UI 的快讯和用户看到的状态都来自同一组 `event_digests` / `event_agent_runs` 表；来源在生成期间变化时拒绝写入旧结果。

事件生成会把仍属于当前 story 的最新有效 revision 文章标题作为 `event_focus`，没有有效 revision 时才回退到初始代表标题；该文章也会优先进入证据材料。非中文标题优先由独立翻译模型直译，独立翻译未配置时复用事件 LLM。服务端强制该译文成为 lead，并保留与当前标题完全匹配的 `T000` 锚点；模型还可附加一条不同 publisher 的异标题/正文证据，但两条都必须由 verifier 分别判定为 `entailed + aligned` 才能计入核心多源支撑。跨语言数字表面形式（如 `two`/`两`、`July`/`7月`）不再由单语 token 规则误杀，而交给这一步强制蕴含核验。若锚点译文不能通过核验，整条事件降为 insufficient，背景 keypoint 不能顶替 lead。人物背景、旁支反应、局部赛况、花絮或事后恢复因此不能替代主事件。快讯由标题锚定核心句加最多一条同样通过核验的补充句组成，总长仍限制在 220 字以内。生成期间 revision、标题、URL、来源名或正文发生变化时，来源快照哈希与运行状态 CAS 会拒绝旧结果；聚类更新还会显式把摘要置回 pending。有积压时 Agent 连续消费任务，空闲时才恢复配置的轮询间隔。

快讯文案使用新闻电讯式的短句，但不模仿特定媒体的固定表达。`verification_status` 表示当前材料状态：

- `corroborated`：核心句 `claim_id=summary` 本身至少有两个可区分来源直接支撑，且未发现材料内冲突；不同附加要点各自引用不同来源不能抬高该标签；
- `developing`：已有多源材料，但实际通过核验的来源或支撑度未达到确认门槛；
- `disputed`：verifier 确认至少一组来源主张或数字不能同时成立；词法推断出的 `CONTRADICT` revision 本身不会直接抬升该标签；
- `corrected`：事件 ledger 已记录更正；
- `insufficient`：只能抽取原文，未完成生成式多源核验。

这些标签不是现实世界真伪概率。C2PA 等来源证明只能验证内容来源和修改历史，不替内容本身作真假判断；Google Fact Check / ClaimReview 只能在存在匹配的专业事实核查时作为额外证据。当前版本因此只声称“材料内多源一致”或“存在分歧”，不输出无证据的“真/假”。

## OSINT 调查

事件详情中的「OSINT 调查」包含本地规则分析和用户触发的原站取证。调查 Agent 只使用已入库材料和 Tantivy 索引，不依赖第三方检索、存档或模型服务。

| 本地工具 | 行为与解释边界 |
| --- | --- |
| 站内检索 | 检索已有文章，过滤静音与失效索引记录；结果只是其他事件或未归类材料线索，不自动合并 |
| 来源审计 | 按发布者、规范链接、完整正文指纹合并已知依赖；不以组数证明真实独立性 |
| 转载线索 | 五字符片段 Jaccard 相似度达到 85% 时提示疑似转载，不自动合并依赖组 |
| 时间核对 | 保留未知日期，标记源报发布时间晚于抓取时间超过 24 小时的材料 |
| 证据片段 | 原样抽取含数字、更正措辞及其他陈述的片段；相同句式数字不同仅提示核对，不宣布矛盾 |
| 本地留存 | 原站直接抓取的精确响应字节、SHA-256 与元数据导出；不使用第三方存档服务 |

预算、算法与 v2 接口见 [本地 OSINT 调查](OSINT_RESEARCH.md)。该调查流程不调用既有新闻摘要模型；现有快讯的历史引文仍按原有快照展示。

「依赖组」按同 publisher、规范链接或正文指纹传递合并，最多分析质量排序前 60 篇未静音材料。老数据以 URL hash 回填的 content hash 不当作正文指纹。不同组仍可能共享通讯社稿件、媒体所有权或原始声明；目前只能发现已知依赖，不能以组数证明独立性。核心支撑组只统计当前可见材料中核心主张的已核验引文，旁支、未核验引文不会抬高数值。引文属于注明时间的分析快照，取证属于另一次页面观察。

抓取由用户主动触发，使用无阅读器凭据的 HTTP 请求，遵循共享重定向/私网检查；即使 `ALLOW_PRIVATE_FETCH=true`，OSINT 仍拒绝私网。响应上限 1 MiB、总取证超时 20 秒；只保留 content-type、content-encoding、etag、last-modified、date 响应头。禁用自动解压，SHA-256 覆盖导出的确切响应体字节，不包含 TLS、HTTP 帧或全部响应头；它能检测该字节串的变化，不能证明事实真实或可信时间戳。仅对未压缩 HTML 提取元数据，非 UTF-8 页面可能出现乱码；始终保留原字节，不渲染 HTML、不执行脚本、不跟进正文外链。

记录随原文/事件保留周期或事件重建清理；需要长期留存时先导出。取证不会重新跑模型或自动发布主张。现有 OpenCLI 白名单和 RSSHub 继续负责平台采集，不增加任意命令执行器，也不要求安装无关侦察工具。

使用 Fake-IP DNS 的本机若将公开域名解析到 `198.18.0.0/15`，会被现有保留地址检查拒绝；部署环境需提供可校验的真实公共解析。不要为取证关闭私网保护。

可在本地校验导出文件：

```python
import hashlib, json
from pathlib import Path

record = json.loads(Path("osint-export.json").read_text())
body = bytes.fromhex(record["body_hex"])
assert len(body) == record["byte_length"]
assert hashlib.sha256(body).hexdigest() == record["sha256"]
```

## 已知边界

- 第二次 verifier 默认仍使用同一 provider/model，是一致性门禁，不是独立事实核查机构。
- 当前 story 聚类仍主要依赖标题相似度和时间窗；事件归并错误会影响多源输入质量。
- 在线聚类与批量重建共用全局事务级 advisory lock，正文写入用同一锁的 shared 模式，模糊相似标题也不会并发创建重复事件或回写旧正文哈希；更新文章会持久标记 `needs_recluster`，成功迁移后同时刷新旧、新簇，worker 启动时也会补扫未聚类或待重聚类条目。
- hostname 去重不能识别通讯社转载链或同一媒体集团，产品文案因此使用“多个来源”，不声称严格独立来源。
- 事件 ledger 和快讯摘要已经持久化；但 `story_id` 在全量重建时仍会变化，因此重建会级联清除并重新生成事件级产物。后续若需要跨重建保存人工核验结论，应引入稳定、可版本化的事件身份。
