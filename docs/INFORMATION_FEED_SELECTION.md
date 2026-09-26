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
