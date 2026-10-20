# Information Feed Selection

## Goal

The `现在` brief should answer “what changed that is worth knowing?” within a fixed attention budget. It is not a newest-first feed and it must not fill a quota with low-signal items.

The selection pipeline is intentionally split into four stages:

1. candidate recall;
2. evidence eligibility;
3. event utility scoring;
4. list-level diversity reranking.

This follows the production pattern described by Reddit and YouTube, where retrieval and filtering happen before ranking and list composition. MMR is used as the first list-level diversity method because it is understandable and works without embeddings or a trained topic ontology.

## Source and synthesis update (2026-09-08)

The preset catalog now starts with primary publications, followed by reporting, specialist analysis, and community feeds. The new `一手发布` group contains:

| Preset | Coverage | Live feed check |
| --- | --- | --- |
| [UN News](https://news.un.org/feed/subscribe/en/news/all/rss.xml) | International institutions and humanitarian developments | 30 entries |
| [Federal Reserve Monetary Policy](https://www.federalreserve.gov/feeds/press_monetary.xml) | Policy announcements and minutes; [official feed directory](https://www.federalreserve.gov/feeds/feeds.htm) | 15 entries |
| [ECB Press Releases](https://www.ecb.europa.eu/rss/press.html) | Policy statements, speeches and press conferences; [official feed directory](https://www.ecb.europa.eu/home/html/rss.en.html) | 15 entries |

Checks used `infofetcher/0.1` and the project's installed `feed-rs` parser; every included entry had a title, link and publication time. These are optional presets, not automatic subscriptions. The initial quality score is an editorial ranking prior, not proof that an institution's claims are true. CERT-EU was excluded from this update: its RSS was reachable, but the current parser lost publication dates containing `CEST`.

- Article summaries and event digests use the existing publisher identity (site hostname or explicit Hub route), feed, content hash and canonical URL to reject repeated evidence. Tracking parameters do not create another source. Muted sources are excluded from automatic supporting material; an explicitly requested primary article remains usable. Source weights affect supporting-material order, while the requested article or event focus stays first.
- Report source quality is averaged once per publisher, using that publisher's highest visible adjusted feed score. Repeated articles cannot multiply its weight. Diversity penalties use publisher identity rather than editable feed titles.
- Local extracts and legacy summaries without a grounding score receive no grounded-summary bonus. During generation, changes to citation title, URL, publisher metadata or body hash invalidate the article-summary snapshot before it is saved.

Deterministic query replay: 13 articles from a publisher scored 50 plus one from a publisher scored 90 previously yielded 53; publisher-balanced scoring yields 70 and remains 70 after adding 20 repeated articles. This is a synthetic regression case, not a production quality measurement. Query checks used an isolated PostgreSQL schema without the unused `pgcrypto`/`pg_trgm` extensions; they do not establish full migration or production performance results.

Validation: 136 Rust tests, Clippy with warnings denied, web typecheck/build, all 60 entries from the three new feeds, and an isolated query replay with 1,201 events / 12,034 visible articles passed. No production subscriptions or model calls were changed by these local checks.

Publisher identity remains a heuristic: separate domains or Hub routes can share ownership or copy the same wire report. URL/content deduplication catches exact copies, not rewritten syndication. The two-stage evidence verifier remains required.

## Source and filter update (2026-09-30)

Three optional presets extend direct-source coverage. Requests used `infofetcher/0.1`; all returned HTTP 200 and parsed with the project's `feed-rs` 2.3.1. Every returned item had a title, link and publication/update timestamp:

| Preset | Coverage | Parsed entries |
| --- | --- | --- |
| [NASA News Releases](https://www.nasa.gov/news-release/feed/) | Space and science announcements; [official directory](https://www.nasa.gov/rss-feeds/) | 10 |
| [USGS Significant Earthquakes](https://earthquake.usgs.gov/earthquakes/feed/v1.0/summary/significant_month.atom) | Significant earthquakes over the past month; [official directory](https://earthquake.usgs.gov/earthquakes/feed/) | 8 |
| [CISA Cybersecurity Advisories](https://www.cisa.gov/cybersecurity-advisories/all.xml) | Official cybersecurity advisories | 30 |

These checks establish retrieval and parser compatibility at the time of testing, not ongoing availability or independent verification. Presets require explicit subscription; no production subscriptions are created by this change.

- Subscription settings support intersecting text search with collection-error, never-succeeded, muted or unmuted states. Unmuted does not mean healthy. Text search covers names, URLs and adapter paths. Recommended sources can be searched by name, category and URL; already-added entries are disabled, and whole-category addition is disabled during search to avoid adding hidden entries.
- List and situation-board layouts share inclusion keywords (all terms), exclusion keywords (any term), exact subscription label and grounded-summary availability filters. Search is case-insensitive literal matching across title, brief text, source labels and Watch names, not regular expressions. The existing topic/workflow/coverage filters further constrain the list. Shared conditions survive switching layouts but are not persisted across page reloads.
- These filters operate only on the returned finite report (or saved items), not on the full article archive and not on candidate recall. Grounded-summary availability is the existing server flag; it is not proof of real-world truth. Source labels identify subscriptions and can be shared by more than one feed.
- Community-only classification now uses publisher-family host boundaries or explicit RSSHub routes rather than editable feed names. Renaming a known community subscription no longer changes its brief gating; unrelated domains merely containing a community name are not classified as that community. This remains a bounded policy for V2EX, NodeSeek and LINUX DO, not a universal source taxonomy.

Validation: 169 Rust tests passed (5 opt-in database tests skipped), Clippy with warnings denied, 12 frontend tests, TypeScript and the production frontend build. Local browser QA uses synthetic API fixtures; it does not establish deployed behavior or live account subscription creation.

## Pipeline update (2026-10-16 to 2026-10-17)

All figures below come from read-only production queries or a local replay of exported production titles. Times are UTC.

### Findings that drove the changes

- In 24 hours, NodeSeek, V2EX and the X "For you" timeline produced about 38% of 2,778 articles. "For you" had the lowest assessed signal-to-noise ratio (0.059). NodeSeek article pages returned 403 for every server request.
- Event clustering chained unrelated stories. Examples: 30 Apple articles inside a story about early prison releases in England; Meta's open-weight model and Tesla's Roadster in one story. Root cause: generic words (`releases`, `second`) and date numbers counted as identity tokens, and `killed`/`kill` were counted twice. Online clustering is single-link and merges every matched cluster, so one bad edge joins two topics.
- Major outlets supplied only feed summaries: NYT (403), FT (403), NPR (connection reset for non-browser clients), Le Monde (402). The assessor scored events lower when the body was missing.
- From 2026-10-16 22:00 the LLM endpoint `cpa.ladymiao.net` returned NXDOMAIN at its authoritative servers. Every assessment, same-event check and title translation failed. Failed assessments still consumed the daily budget of 600, and multi-source events fell from 10% to 5% of the 24-hour total. On 2026-10-17 20:14 production switched to `cpa1.cat-infra.com`; within five minutes 46 LLM calls succeeded with no transport errors, 35 events were assessed and the first 30 outage articles were re-checked.

### Changes

| Area | Change | Commit |
| --- | --- | --- |
| Clustering | Identity tokens ignore inflected stop words, ordinals and dates 1–31; shared-token counts use stems; more country and US-state geography markers. `recluster-recent` accepts `RECLUSTER_MIN_STORY_SOURCES` to rebuild only large clusters. | `b9f4b47` |
| Extraction | NodeSeek uses feed content (as V2EX does). NPR articles are fetched from `text.npr.org`. | `b9f4b47`, `f94937e` |
| Assessment input | When the representative body is under 600 characters, the longest body from another source in the same event is appended with attribution. | `f94937e` |
| Sources | Web Watch gains a `link_prefix` article-list mode, used for Anthropic Research. Reading view links to removepaywall only when extraction failed and only on user click; paywalled full text is not fetched automatically. | `1496439` |
| Resilience | The daily assessment budget counts only successful assessments. A transport error stops the turn without recording a failure. Same-event checks that failed during an outage are retried for 48 hours. | this update |

Production operations (no code): muted X "For you"; added 28 free sources (DW, PBS, Euronews, Politico Europe, Foreign Policy, The Diplomat, Meduza, CNBC, WHO, BBC/RFI/DW Chinese, CBC, ABC Australia, AP summaries, Lianhe Zaobao, Nikkei Asia summaries, Nature, MIT Technology Review, Quanta, Google DeepMind, Google Research, The Conversation, Rest of World, OpenAI Research, Anthropic Research); upgraded RSSHub from the 2026-07-23 image to 2026-10-17 after checking that all 15 existing routes returned the same item counts; rebuilt 1,106 articles in clusters with six or more sources.

### Verified results

- Replay of 9,626 production articles (72 hours): the known mixed clusters disappeared and multi-source clusters changed from 466 to 463. In production, after the rebuild, no 72-hour story contained both topics of any known bad pair.
- The 28 new sources fetched without failure. Most delivered full text for nearly all items; Times of India, PBS, AP and Nikkei deliver mostly summaries.
- During the LLM outage the assessor stops after one attempt per turn and writes no failure rows.

### Open items

- Explicit feedback is almost unused (six event actions in total), so preference transfer has no signal. Implicit signals (open, save) need exposure logging first.
- Feed titles are overwritten from the feed on every fetch, so manual renames do not persist.
- Washington Post World (feed 502) and LINUX DO (Cloudflare 403, no RSSHub route) remain failing with daily back-off.

## Verified Baseline

Read-only production snapshot on 2026-07-14:

- 1,700 events were observed in 24 hours, while the previous API scored only the newest 120, all from roughly the latest two hours.
- About 78% of those 120 candidates came from NodeSeek, LINUX DO, or V2EX.
- 115–116 of 120 candidates were single-source and 118 had confidence below 55.
- All 17 feeds still had the default `quality_score = 50`; source preference weights and Watches had no live signal.
- 12-item proxy replay of the old selector produced six Guardian items and eleven single-source items. Topic bucketing reduced one concentration but created another.
- `title.contains("ai")` placed about 12 non-AI candidates into the technology bucket.
- Two representative articles lagged their event's latest observation by more than 24 hours; the largest lag was 74.1 hours.
- In 24 hours, 50 identical `content_hash` groups and 42 identical `url_hash` groups crossed story-cluster boundaries.

The ingestion layer was healthy. The bottleneck was candidate selection, evidence calibration, and list composition.

## Selection Policy v1

### Candidate recall

- Score up to 2,000 recent events instead of 120.
- Respect muted sources before choosing the representative article.
- Prefer the latest visible revision article, then article quality and publication time.
- Apply explicit `source_feedback.weight` to source quality.

### Evidence eligibility

An event is eligible when one of these conditions is true:

- it matches an explicit Watch and has total score at least 55;
- it has at least two independent publishers and total score at least 58;
- it is a correction or contradiction from a source-quality score of at least 55 and total score at least 58;
- it has source-quality score at least 70 and total score at least 60;
- it is from a non-community source and total score at least 60;
- it is community-only, but has total score at least 68, impact at least 65, and confidence at least 40.

`irrelevant` and `bad_cluster` are hard exclusions. `known` remains a workflow state rather than a negative-interest signal and receives a list-level demotion so completed events do not normally consume the pending budget.

The initial community boundary covers the currently configured NodeSeek, V2EX, and LINUX DO feeds. This is a product policy for the finite world brief, not a claim that community content is intrinsically low quality.

### Utility score

The existing impact, confidence, relevance, and total-score structure is retained to keep the first rollout attributable. Two signals change:

- freshness is continuous: `100 × 2^(-age_hours / 12)`, with bounded update and correction bonuses;
- novelty is based on revision type and recent changes, with only a small repetition penalty, instead of rewarding events merely because they have fewer articles.

ASCII Watch terms use token boundaries, and multi-term Watches require at least half the terms (minimum two) to match. This prevents `AI` from matching `chair` and prevents a full tracked title from matching on one generic word.

`more`, `less`, and `irrelevant` feedback now transfer a bounded preference adjustment to later events with overlapping title signals. `known` and `bad_cluster` do not train interest.

### Diversity reranking

The final list is built greedily. Each remaining candidate starts with its utility score and receives:

- bonuses for independent corroboration, grounded summary availability, Watch matches, corrections, and contradictions;
- a title-similarity penalty against already selected events;
- hard exclusion for repeated representative `content_hash` or `url_hash` values across broken clusters;
- an increasing topic concentration penalty;
- an increasing publisher repetition penalty.

Chinese titles use CJK bigrams for duplicate detection. ASCII topic markers use token boundaries instead of substring matching.

This is an interpretable MMR-style reranker. It deliberately uses soft penalties so a brief may still contain multiple events from one source when the alternative is filling the brief with weaker evidence.

## Feedback Semantics

- `more`: increase related-event relevance.
- `less`: remove the current item from pending and lower related-event relevance.
- `irrelevant`: hard-exclude the current item and apply a stronger related-event penalty.
- `known`: mark the workflow item complete; do not treat it as dislike.
- `bad_cluster`: exclude the current item from the brief; do not train topic interest.

The frontend counts `less`, `irrelevant`, and `bad_cluster` as processed, so the finite brief can still be completed honestly.

## Validation and Next Phase

The first phase is covered by deterministic unit tests for evidence gating, continuous freshness, revision novelty, Watch boundaries, Chinese duplicate detection, preference transfer, and publisher/topic reranking.

The next data-model phase should add append-only brief exposures and feedback events before attempting bandits or learned ranking. Required fields include request ID, candidate set, feature snapshot, exclusion reason, final position, rerank score, shown time, and explicit interaction.

Primary evaluation metrics:

- useful-action rate at K (`more`, Save, Watch creation);
- negative-feedback rate at K;
- independent-source and topic coverage at K;
- maximum pairwise title/entity similarity;
- single-source share and bad-cluster rate;
- time-to-first-surface for events later corroborated by multiple publishers;
- brief completion time and abandonment rate.

Do not use non-clicks as negative feedback until exposure logging and off-policy evaluation exist.

## Research Sources

- [Goldstein & Carbonell: MMR for diversity-based reranking](https://aclanthology.org/X98-1025/)
- [Google Research: accuracy and diversity metrics for recommender systems](https://research.google/pubs/towards-unified-metrics-for-accuracy-and-diversity-for-recommender-systems/)
- [Google News: topic authority](https://developers.google.com/search/blog/2023/05/understanding-news-topic-authority)
- [YouTube: recommendation satisfaction and authoritative information](https://blog.youtube/inside-youtube/on-youtubes-recommendation-system/)
- [Reddit: candidate filtering, ranking, and diversity](https://support.reddithelp.com/hc/en-us/articles/23511859482388-Reddit-s-Approach-to-Content-Recommendations)
- [Yahoo Research: contextual-bandit news recommendation](https://arxiv.org/abs/1003.0146)

## Source update (2026-10-20)

Reference: an AI source list shared on X (snwiki238337, 2026-10-20). Only research and primary-reporting sources were taken; tool directories, tutorial YouTube channels, podcasts and promotional newsletters were skipped. All candidates were requested from the production host.

| Added to production | First fetch |
| --- | --- |
| Import AI, Interconnects, SemiAnalysis, Latent Space (AI 研究) | 20 items each |
| ChinaTalk, THE DECODER, 量子位 via RSSHub `qbitai/category/资讯` (科技媒体) | 20 / 10 / 10 |
| The Batch via RSSHub `deeplearning/the-batch` (AI 研究) | 16 |
| X `x-ai-voices`: Hassabis, LeCun, Andrew Ng, Sutskever, Hinton, Bengio, AI at Meta, Runway, Chubby, Rohan Paul, Xudong Han | 60 |

Rejected: 机器之心 RSSHub routes (404), GitHub Trending (503), a16z feed (404), Meta AI blog route (latest item July 2026), Hugging Face Blog (feed returns 882 historical items and the first fetch does not filter by age), aggregators with heavy overlap (TLDR AI, AIbase).

Fix found during this update: `POST /api/feeds` and `POST /api/social-sources` panicked since the editorial layer, because their `RETURNING` lists lacked `editorial_profile` and `quality_assessed_events`. The insert committed before the panic, so the client saw an error while the feed was created. Rollback suffix `before-sources-20261020T035334Z`; env backup `.env.homeserver.before-xvoices-20261020T034944Z`.
