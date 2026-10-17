//! 事件层向量召回：新文章按规则自成一簇时，用标题向量在 48 小时窗口内找其他信源的相近报道，
//! 再由模型确认是否同一事件，确认后挂到对方的簇。只做“召回 + 确认”，不拆分、不合并现有簇。
//!
//! 阈值来自 2026-10-14 离线实验（EmbeddingGemma 2、SentenceSimilarity 前缀、只用标题，
//! 7 天 15,960 篇）：余弦 ≥ 0.90 的跨语言新增配对 88% 是同一事件；同语言 0.90–0.94 只有 60%，
//! ≥ 0.94 为 96%。向量只负责召回，最终以模型确认为准。

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
};

use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::Row;
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    intelligence,
    llm::{self, SameEventCandidate},
    state::AppState,
};

const PROMPT_PREFIX: &str = "task: sentence similarity | query: ";
const WINDOW: Duration = Duration::hours(48);
const CROSS_LANGUAGE_MIN_COSINE: f32 = 0.90;
const SAME_LANGUAGE_MIN_COSINE: f32 = 0.94;
const MAX_CANDIDATE_ARTICLES: usize = 20;
const MAX_CANDIDATE_STORIES: usize = 3;
const EMBED_BATCH: usize = 32;
const BACKFILL_LIMIT: i64 = 8_000;

struct Entry {
    vector: Vec<f32>,
    at: DateTime<Utc>,
    feed_id: Uuid,
    language: Option<String>,
}

#[derive(Default)]
struct Cache {
    loaded: bool,
    entries: HashMap<Uuid, Entry>,
}

// 窗口内的向量常驻进程内存（约 5 千篇 × 768 维 ≈ 15 MB）；进程重启后从表里重新载入。
static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Mutex::default);
static EMBED_PERMITS: Semaphore = Semaphore::const_new(2);
static LOAD_PERMIT: Semaphore = Semaphore::const_new(1);

pub fn enabled(state: &AppState) -> bool {
    state.config.embedding_base_url.is_some() && !state.config.llm_api_key.trim().is_empty()
}

/// 在后台为新文章做召回与确认。失败只记日志，不影响抓取任务。
pub fn spawn_link(state: &AppState, article_id: Uuid) {
    if !enabled(state) {
        return;
    }
    let state = state.clone();
    tokio::spawn(async move {
        if let Err(error) = link_new_article(&state, article_id).await {
            tracing::warn!(%article_id, %error, "semantic story link failed");
        }
    });
}

/// 服务商故障期间同事件判断全部失败的文章，入库后不会再判断；恢复后在这里补做。
/// 只取 48 小时内、所有判断都失败、仍是单篇簇的文章；再遇到连接错误就停止本轮。
pub async fn retry_failed_links(state: &AppState) -> AppResult<usize> {
    if !enabled(state) {
        return Ok(0);
    }
    let ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT d.article_id
         FROM story_link_decisions d
         JOIN articles a ON a.id = d.article_id
         WHERE d.created_at >= now() - interval '48 hours'
           AND a.story_id IS NOT NULL
           AND (SELECT count(*) FROM articles x WHERE x.story_id = a.story_id) = 1
         GROUP BY d.article_id
         HAVING bool_and(d.verdict = 'error')
            AND max(d.created_at) < now() - interval '30 minutes'
         ORDER BY max(d.created_at) DESC
         LIMIT 30",
    )
    .fetch_all(&state.pool)
    .await?;
    let mut retried = 0;
    for article_id in ids {
        match link_new_article(state, article_id).await {
            Ok(()) => retried += 1,
            Err(AppError::Reqwest(error)) => {
                tracing::warn!(%error, retried, "semantic link retry stopped, provider unreachable");
                break;
            }
            Err(error) => tracing::warn!(%article_id, %error, "semantic link retry failed"),
        }
    }
    Ok(retried)
}

async fn link_new_article(state: &AppState, article_id: Uuid) -> AppResult<()> {
    ensure_loaded(state).await?;
    let Some(row) = sqlx::query(
        "SELECT a.title, a.language, a.feed_id, a.story_id, a.published_at,
                COALESCE(a.published_at, a.fetched_at) AS reference_at, f.title AS source,
                (SELECT count(*) FROM articles x WHERE x.story_id = a.story_id) AS members
         FROM articles a
         JOIN feeds f ON f.id = a.feed_id
         WHERE a.id = $1",
    )
    .bind(article_id)
    .fetch_optional(&state.pool)
    .await?
    else {
        return Ok(());
    };
    let title = row.get::<String, _>("title");
    let language = row.get::<Option<String>, _>("language");
    let feed_id = row.get::<Uuid, _>("feed_id");
    let reference_at = row.get::<DateTime<Utc>, _>("reference_at");
    let Some(vector) = embed_and_store(state, &[(article_id, title.clone())])
        .await?
        .pop()
    else {
        return Ok(());
    };
    remember(article_id, vector.clone(), reference_at, feed_id, language.clone());

    let Some(story_id) = row.get::<Option<Uuid>, _>("story_id") else {
        return Ok(());
    };
    // 规则已经把它归进多篇报道的簇时，不再改动。
    if row.get::<i64, _>("members") > 1 {
        return Ok(());
    }
    let hits = nearest(article_id, &vector, reference_at, feed_id, language.as_deref());
    if hits.is_empty() {
        return Ok(());
    }
    let candidates = candidate_stories(state, story_id, &hits).await?;
    if candidates.is_empty() {
        return Ok(());
    }

    let report = SameEventCandidate {
        id: "report".to_string(),
        title,
        source: row.get("source"),
        published_at: row.get("published_at"),
    };
    let offered = candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| SameEventCandidate {
            id: format!("c{}", index + 1),
            title: candidate.title.clone(),
            source: candidate.source.clone(),
            published_at: candidate.published_at,
        })
        .collect::<Vec<_>>();
    let decision = {
        let _permit = state
            .llm_semaphore
            .acquire()
            .await
            .map_err(|error| AppError::Anyhow(error.into()))?;
        llm::match_same_event_with_llm(&state.config, &state.http, &report, &offered).await
    };
    let matched = match &decision {
        Ok(Some(id)) => offered.iter().position(|candidate| &candidate.id == id),
        Ok(None) => None,
        Err(_) => None,
    };
    for (index, candidate) in candidates.iter().enumerate() {
        let verdict = match (&decision, matched) {
            (Err(_), _) => "error",
            (Ok(_), Some(chosen)) if chosen == index => "linked",
            _ => "rejected",
        };
        sqlx::query(
            "INSERT INTO story_link_decisions
               (article_id, candidate_article_id, candidate_story_id, cosine,
                cross_language, verdict, model)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(article_id)
        .bind(candidate.article_id)
        .bind(candidate.story_id)
        .bind(candidate.cosine)
        .bind(candidate.cross_language)
        .bind(verdict)
        .bind(&state.config.embedding_model)
        .execute(&state.pool)
        .await?;
    }
    decision?;
    if let Some(index) = matched {
        let target = candidates[index].story_id;
        intelligence::link_article_to_story(state, article_id, target).await?;
        tracing::info!(%article_id, story_id = %target, cosine = candidates[index].cosine,
            "article linked to same-event story by semantic recall");
    }
    Ok(())
}

struct Hit {
    article_id: Uuid,
    cosine: f32,
    cross_language: bool,
}

fn nearest(
    article_id: Uuid,
    vector: &[f32],
    reference_at: DateTime<Utc>,
    feed_id: Uuid,
    language: Option<&str>,
) -> Vec<Hit> {
    let cache = CACHE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut hits = cache
        .entries
        .iter()
        .filter(|(id, entry)| {
            **id != article_id
                && entry.feed_id != feed_id
                && (entry.at - reference_at).abs() <= WINDOW
        })
        .filter_map(|(id, entry)| {
            let cross_language = is_cross_language(language, entry.language.as_deref());
            let cosine = dot(vector, &entry.vector);
            (cosine >= min_cosine(cross_language)).then_some(Hit {
                article_id: *id,
                cosine,
                cross_language,
            })
        })
        .collect::<Vec<_>>();
    hits.sort_by(|left, right| right.cosine.total_cmp(&left.cosine));
    hits.truncate(MAX_CANDIDATE_ARTICLES);
    hits
}

struct CandidateStory {
    story_id: Uuid,
    article_id: Uuid,
    title: String,
    source: String,
    published_at: Option<DateTime<Utc>>,
    cosine: f32,
    cross_language: bool,
}

/// 每个簇取余弦最高的一篇作为代表，最多给模型 3 个簇。
async fn candidate_stories(
    state: &AppState,
    own_story_id: Uuid,
    hits: &[Hit],
) -> AppResult<Vec<CandidateStory>> {
    let ids = hits.iter().map(|hit| hit.article_id).collect::<Vec<_>>();
    let rows = sqlx::query(
        "SELECT a.id, a.story_id, a.title, a.published_at, f.title AS source
         FROM articles a
         JOIN feeds f ON f.id = a.feed_id
         WHERE a.id = ANY($1) AND a.story_id IS NOT NULL AND a.story_id <> $2",
    )
    .bind(&ids)
    .bind(own_story_id)
    .fetch_all(&state.pool)
    .await?;
    let by_article = rows
        .into_iter()
        .map(|row| (row.get::<Uuid, _>("id"), row))
        .collect::<HashMap<_, _>>();
    let mut stories = Vec::<CandidateStory>::new();
    for hit in hits {
        let Some(row) = by_article.get(&hit.article_id) else {
            continue;
        };
        let story_id = row.get::<Uuid, _>("story_id");
        if stories.iter().any(|story| story.story_id == story_id) {
            continue;
        }
        stories.push(CandidateStory {
            story_id,
            article_id: hit.article_id,
            title: row.get("title"),
            source: row.get("source"),
            published_at: row.get("published_at"),
            cosine: hit.cosine,
            cross_language: hit.cross_language,
        });
        if stories.len() == MAX_CANDIDATE_STORIES {
            break;
        }
    }
    Ok(stories)
}

/// 首次使用时载入窗口内已有向量，并在后台补齐窗口内缺少向量的文章。
async fn ensure_loaded(state: &AppState) -> AppResult<()> {
    if CACHE.lock().unwrap_or_else(|p| p.into_inner()).loaded {
        return Ok(());
    }
    let _permit = LOAD_PERMIT
        .acquire()
        .await
        .map_err(|error| AppError::Anyhow(error.into()))?;
    if CACHE.lock().unwrap_or_else(|p| p.into_inner()).loaded {
        return Ok(());
    }
    let rows = sqlx::query(
        "SELECT e.article_id, e.vector, a.feed_id, a.language,
                COALESCE(a.published_at, a.fetched_at) AS reference_at
         FROM article_embeddings e
         JOIN articles a ON a.id = e.article_id
         WHERE e.model = $1 AND COALESCE(a.published_at, a.fetched_at) >= $2",
    )
    .bind(&state.config.embedding_model)
    .bind(Utc::now() - WINDOW * 2)
    .fetch_all(&state.pool)
    .await?;
    {
        let mut cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
        for row in rows {
            cache.entries.insert(
                row.get("article_id"),
                Entry {
                    vector: row.get("vector"),
                    at: row.get("reference_at"),
                    feed_id: row.get("feed_id"),
                    language: row.get("language"),
                },
            );
        }
        cache.loaded = true;
    }
    let state = state.clone();
    tokio::spawn(async move {
        match backfill(&state).await {
            Ok(count) if count > 0 => tracing::info!(count, "semantic vectors backfilled"),
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "semantic vector backfill failed"),
        }
    });
    Ok(())
}

async fn backfill(state: &AppState) -> AppResult<usize> {
    let rows = sqlx::query(
        "SELECT a.id, a.title, a.feed_id, a.language,
                COALESCE(a.published_at, a.fetched_at) AS reference_at
         FROM articles a
         WHERE COALESCE(a.published_at, a.fetched_at) >= $2
           AND NOT EXISTS (
             SELECT 1 FROM article_embeddings e WHERE e.article_id = a.id AND e.model = $1
           )
         ORDER BY COALESCE(a.published_at, a.fetched_at) DESC
         LIMIT $3",
    )
    .bind(&state.config.embedding_model)
    .bind(Utc::now() - WINDOW * 2)
    .bind(BACKFILL_LIMIT)
    .fetch_all(&state.pool)
    .await?;
    let mut count = 0;
    for chunk in rows.chunks(EMBED_BATCH) {
        let texts = chunk
            .iter()
            .map(|row| (row.get::<Uuid, _>("id"), row.get::<String, _>("title")))
            .collect::<Vec<_>>();
        let vectors = embed_and_store(state, &texts).await?;
        for (row, vector) in chunk.iter().zip(vectors) {
            remember(
                row.get("id"),
                vector,
                row.get("reference_at"),
                row.get("feed_id"),
                row.get("language"),
            );
        }
        count += chunk.len();
    }
    Ok(count)
}

fn remember(
    article_id: Uuid,
    vector: Vec<f32>,
    at: DateTime<Utc>,
    feed_id: Uuid,
    language: Option<String>,
) {
    let mut cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    let oldest = Utc::now() - WINDOW * 2;
    if cache.entries.len().is_multiple_of(256) {
        cache.entries.retain(|_, entry| entry.at >= oldest);
    }
    cache.entries.insert(
        article_id,
        Entry {
            vector,
            at,
            feed_id,
            language,
        },
    );
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingItem>,
}

#[derive(Deserialize)]
struct EmbeddingItem {
    index: usize,
    embedding: Vec<f32>,
}

/// 调用向量服务，归一化后写入 `article_embeddings`，按输入顺序返回向量。
async fn embed_and_store(state: &AppState, texts: &[(Uuid, String)]) -> AppResult<Vec<Vec<f32>>> {
    let Some(base_url) = state.config.embedding_base_url.as_deref() else {
        return Ok(Vec::new());
    };
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let inputs = texts
        .iter()
        .map(|(_, title)| embedding_input(title))
        .collect::<Vec<_>>();
    let response = {
        let _permit = EMBED_PERMITS
            .acquire()
            .await
            .map_err(|error| AppError::Anyhow(error.into()))?;
        state
            .http
            .post(format!("{}/embeddings", base_url.trim_end_matches('/')))
            .timeout(state.config.embedding_timeout)
            .json(&json!({ "model": state.config.embedding_model, "input": inputs }))
            .send()
            .await
            .map_err(|error| AppError::Upstream(format!("向量服务请求失败: {error}")))?
    };
    if !response.status().is_success() {
        return Err(AppError::Upstream(format!(
            "向量服务返回 {}",
            response.status()
        )));
    }
    let mut body = response
        .json::<EmbeddingResponse>()
        .await
        .map_err(|error| AppError::Upstream(format!("向量服务响应无法解析: {error}")))?;
    if body.data.len() != texts.len() {
        return Err(AppError::Upstream("向量服务返回的条数与请求不一致".to_string()));
    }
    body.data.sort_by_key(|item| item.index);
    let vectors = body
        .data
        .into_iter()
        .map(|item| normalize(item.embedding))
        .collect::<Vec<_>>();
    for ((article_id, _), (input, vector)) in texts.iter().zip(inputs.iter().zip(&vectors)) {
        sqlx::query(
            "INSERT INTO article_embeddings (article_id, model, text_hash, vector)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (article_id) DO UPDATE SET
               model = EXCLUDED.model, text_hash = EXCLUDED.text_hash,
               vector = EXCLUDED.vector, created_at = now()",
        )
        .bind(article_id)
        .bind(&state.config.embedding_model)
        .bind(hex::encode(Sha256::digest(input.as_bytes())))
        .bind(vector)
        .execute(&state.pool)
        .await?;
    }
    Ok(vectors)
}

fn embedding_input(title: &str) -> String {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("{PROMPT_PREFIX}{title}")
}

fn normalize(mut vector: Vec<f32>) -> Vec<f32> {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        vector.iter_mut().for_each(|value| *value /= norm);
    }
    vector
}

fn dot(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() {
        return 0.0;
    }
    left.iter().zip(right).map(|(a, b)| a * b).sum()
}

/// 两边语言都已知且不同才算跨语言；未知语言按同语言处理，用更严的阈值。
fn is_cross_language(left: Option<&str>, right: Option<&str>) -> bool {
    matches!((left, right), (Some(left), Some(right)) if !left.eq_ignore_ascii_case(right))
}

fn min_cosine(cross_language: bool) -> f32 {
    if cross_language {
        CROSS_LANGUAGE_MIN_COSINE
    } else {
        SAME_LANGUAGE_MIN_COSINE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_depend_on_language_pair() {
        assert!(is_cross_language(Some("zh"), Some("en")));
        assert!(!is_cross_language(Some("en"), Some("EN")));
        assert!(!is_cross_language(None, Some("en")));
        assert_eq!(min_cosine(true), 0.90);
        assert_eq!(min_cosine(false), 0.94);
    }

    #[test]
    fn embedding_input_uses_similarity_prefix_and_collapses_whitespace() {
        assert_eq!(
            embedding_input("  US  sanctions\nIran "),
            "task: sentence similarity | query: US sanctions Iran"
        );
    }

    /// 真实链路：迁移过的测试库 + 本地向量服务 + 可用的 LLM。
    #[tokio::test]
    #[ignore = "requires RSS_OSINT_TEST_DATABASE_URL, EMBEDDING_BASE_URL and a working LLM endpoint"]
    async fn links_cross_language_report_to_existing_story() {
        use std::sync::Arc;

        use tokio::sync::mpsc;

        use crate::config::Config;

        let pool = sqlx::PgPool::connect(&std::env::var("RSS_OSINT_TEST_DATABASE_URL").unwrap())
            .await
            .unwrap();
        let config = (*Config::from_env().unwrap()).clone();
        assert!(config.embedding_base_url.is_some());
        let index = tempfile::tempdir().unwrap();
        let (worker_tx, _) = mpsc::channel(1);
        let state = AppState {
            config: Arc::new(config),
            pool: pool.clone(),
            http: reqwest::Client::new(),
            search: Arc::new(crate::search::SearchIndex::open(index.path()).unwrap()),
            worker_tx,
            login_limiter: Arc::new(crate::security::LoginRateLimiter::default()),
            opencli_semaphore: Arc::new(Semaphore::new(1)),
            llm_semaphore: Arc::new(Semaphore::new(1)),
            translation_semaphore: Arc::new(Semaphore::new(1)),
        };
        // 清掉上次失败运行留下的同名数据，否则规则会先把同标题文章归到一起。
        sqlx::query("DELETE FROM feeds WHERE title IN ('wire-en.example', 'wire-zh.example')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "DELETE FROM story_clusters sc
             WHERE NOT EXISTS (SELECT 1 FROM articles a WHERE a.story_id = sc.id)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let run = Uuid::new_v4();
        let mut feeds = Vec::new();
        for host in ["wire-en.example", "wire-zh.example"] {
            let feed: Uuid = sqlx::query_scalar(
                "INSERT INTO feeds (title, feed_url, site_url) VALUES ($1, $2, $2) RETURNING id",
            )
            .bind(host)
            .bind(format!("https://{host}/{run}"))
            .fetch_one(&pool)
            .await
            .unwrap();
            feeds.push(feed);
        }
        let insert = |feed: Uuid, title: &'static str, language: &'static str| {
            let pool = pool.clone();
            async move {
                let url = format!("https://news.example/{}", Uuid::new_v4());
                sqlx::query_scalar::<_, Uuid>(
                    "INSERT INTO articles
                       (feed_id, guid, url, url_hash, canonical_url, content_hash, title,
                        language, published_at)
                     VALUES ($1, $2, $2, $2, $2, $2, $3, $4, now()) RETURNING id",
                )
                .bind(feed)
                .bind(url)
                .bind(title)
                .bind(language)
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };
        let story_of = |id: Uuid| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, Uuid>("SELECT story_id FROM articles WHERE id = $1")
                    .bind(id)
                    .fetch_one(&pool)
                    .await
                    .unwrap()
            }
        };

        let english = insert(
            feeds[0],
            "Elon Musk has donated over $276 million to Republican midterm efforts",
            "en",
        )
        .await;
        intelligence::assign_story_cluster(&state, english).await.unwrap();
        link_new_article(&state, english).await.unwrap();

        let chinese = insert(feeds[1], "马斯克7月以来向共和党捐款超2.76亿美元，成美国中期选举史上最大金主", "zh").await;
        intelligence::assign_story_cluster(&state, chinese).await.unwrap();
        let before = story_of(chinese).await;
        assert_ne!(before, story_of(english).await, "rules alone should not join these titles");
        link_new_article(&state, chinese).await.unwrap();
        assert_eq!(story_of(chinese).await, story_of(english).await);
        let orphan_left = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM story_clusters WHERE id = $1)",
        )
        .bind(before)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(!orphan_left, "the emptied single-article story is removed");

        let unrelated = insert(feeds[1], "台风登陆广东 多地停课", "zh").await;
        intelligence::assign_story_cluster(&state, unrelated).await.unwrap();
        link_new_article(&state, unrelated).await.unwrap();
        assert_ne!(story_of(unrelated).await, story_of(english).await);

        let verdicts = sqlx::query_scalar::<_, String>(
            "SELECT verdict FROM story_link_decisions WHERE article_id = ANY($1) ORDER BY id",
        )
        .bind(vec![english, chinese, unrelated])
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(verdicts, vec!["linked".to_string()]);
        sqlx::query("DELETE FROM feeds WHERE id = ANY($1)")
            .bind(&feeds)
            .execute(&pool)
            .await
            .unwrap();
    }

    #[test]
    fn nearest_filters_feed_window_and_threshold() {
        let feed = Uuid::from_u128(1);
        let other_feed = Uuid::from_u128(2);
        let now = Utc::now();
        let unit = |x: f32| normalize(vec![x, (1.0 - x * x).sqrt()]);
        let ids = (10..15).map(Uuid::from_u128).collect::<Vec<_>>();
        {
            let mut cache = CACHE.lock().unwrap();
            cache.entries.clear();
            // 跨语言 0.92：命中
            cache.entries.insert(ids[0], Entry { vector: unit(0.92), at: now, feed_id: other_feed, language: Some("zh".into()) });
            // 同语言 0.92：低于 0.94，不命中
            cache.entries.insert(ids[1], Entry { vector: unit(0.92), at: now, feed_id: other_feed, language: Some("en".into()) });
            // 同信源：不命中
            cache.entries.insert(ids[2], Entry { vector: unit(0.99), at: now, feed_id: feed, language: Some("zh".into()) });
            // 超出 48 小时：不命中
            cache.entries.insert(ids[3], Entry { vector: unit(0.99), at: now - Duration::hours(49), feed_id: other_feed, language: Some("zh".into()) });
            // 同语言 0.96：命中
            cache.entries.insert(ids[4], Entry { vector: unit(0.96), at: now, feed_id: other_feed, language: Some("en".into()) });
        }
        let hits = nearest(Uuid::from_u128(99), &unit(1.0), now, feed, Some("en"));
        let found = hits.iter().map(|hit| hit.article_id).collect::<Vec<_>>();
        assert_eq!(found, vec![ids[4], ids[0]]);
        assert!(hits[1].cross_language);
        CACHE.lock().unwrap().entries.clear();
    }
}
