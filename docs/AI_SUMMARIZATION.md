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
- 同一 story 中存在不同 hostname、feed 和 `content_hash` 的材料时：最多取 5 个来源，使用 `grounded-event-v3`，摘要仍归属于请求文章，不持久依赖不稳定的 `story_id`。

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

## 已知边界

- 第二次 verifier 默认仍使用同一 provider/model，是一致性门禁，不是独立事实核查机构。
- 当前 story 聚类仍主要依赖标题相似度和时间窗；事件归并错误会影响多源输入质量。
- 在线聚类与批量重建共用全局事务级 advisory lock，正文写入用同一锁的 shared 模式，模糊相似标题也不会并发创建重复事件或回写旧正文哈希；更新文章会持久标记 `needs_recluster`，成功迁移后同时刷新旧、新簇，worker 启动时也会补扫未聚类或待重聚类条目。
- hostname 去重不能识别通讯社转载链或同一媒体集团，产品文案因此使用“多个来源”，不声称严格独立来源。
- 事件 ledger 和快讯摘要已经持久化；但 `story_id` 在全量重建时仍会变化，因此重建会级联清除并重新生成事件级产物。后续若需要跨重建保存人工核验结论，应引入稳定、可版本化的事件身份。
