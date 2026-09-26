use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Duration, Utc};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, QueryBuilder, Row, Transaction};
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    state::AppState,
};

const STORY_ASSIGNMENT_LOCK_KEY: i64 = 0x5253_5354_4f52_5901;
const STORY_WINDOW: Duration = Duration::days(4);
const SAME_SOURCE_WINDOW: Duration = Duration::hours(12);
const CROSS_LANGUAGE_WINDOW: Duration = Duration::hours(12);
const MAX_APPROXIMATE_CANDIDATES: usize = 2_000;

pub async fn lock_story_generation_for_article_write(
    transaction: &mut Transaction<'_, Postgres>,
) -> AppResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock_shared($1)")
        .bind(STORY_ASSIGNMENT_LOCK_KEY)
        .fetch_one(&mut **transaction)
        .await?;
    Ok(())
}

pub async fn lock_story_generation_for_metadata_write(
    transaction: &mut Transaction<'_, Postgres>,
) -> AppResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(STORY_ASSIGNMENT_LOCK_KEY)
        .fetch_one(&mut **transaction)
        .await?;
    Ok(())
}

pub async fn refresh_stories_for_feed_change(
    transaction: &mut Transaction<'_, Postgres>,
    feed_id: Uuid,
    refresh_cluster_metrics: bool,
) -> AppResult<()> {
    let story_ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT DISTINCT story_id
         FROM articles
         WHERE feed_id = $1 AND story_id IS NOT NULL",
    )
    .bind(feed_id)
    .fetch_all(&mut **transaction)
    .await?;
    refresh_story_clusters_after_source_change(transaction, &story_ids, refresh_cluster_metrics)
        .await
}

pub async fn refresh_story_clusters_after_source_change(
    transaction: &mut Transaction<'_, Postgres>,
    story_ids: &[Uuid],
    refresh_cluster_metrics: bool,
) -> AppResult<()> {
    if story_ids.is_empty() {
        return Ok(());
    }
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(STORY_ASSIGNMENT_LOCK_KEY)
        .fetch_one(&mut **transaction)
        .await?;
    if refresh_cluster_metrics {
        sqlx::query(
            "WITH stats AS (
               SELECT a.story_id,
                      COUNT(DISTINCT CASE
                        WHEN f.hub_adapter IS NOT NULL AND f.hub_path IS NOT NULL
                          THEN lower(f.hub_adapter || ':' || f.hub_path)
                        ELSE lower(regexp_replace(
                          COALESCE(
                            substring(COALESCE(NULLIF(f.site_url, ''), f.feed_url)
                              from '^[a-zA-Z][a-zA-Z0-9+.-]*://([^/:?#]+)'),
                            COALESCE(NULLIF(f.site_url, ''), f.feed_url)
                          ),
                          '^www[.]', ''
                        ))
                      END)::integer AS source_count,
                      ROUND(AVG((a.quality_score + f.quality_score)::numeric / 2))::integer
                        AS quality_score,
                      MIN(a.published_at) AS first_published_at,
                      MAX(a.published_at) AS last_published_at
               FROM articles a
               JOIN feeds f ON f.id = a.feed_id
               WHERE a.story_id = ANY($1)
               GROUP BY a.story_id
             )
             UPDATE story_clusters sc
             SET source_count = stats.source_count,
                 quality_score = stats.quality_score,
                 first_published_at = stats.first_published_at,
                 last_published_at = stats.last_published_at,
                 updated_at = clock_timestamp()
             FROM stats
             WHERE sc.id = stats.story_id",
        )
        .bind(story_ids)
        .execute(&mut **transaction)
        .await?;
        sqlx::query(
            "DELETE FROM story_clusters sc
             WHERE sc.id = ANY($1)
               AND NOT EXISTS (SELECT 1 FROM articles a WHERE a.story_id = sc.id)
               AND NOT EXISTS (SELECT 1 FROM event_feedback ef WHERE ef.story_id = sc.id)",
        )
        .bind(story_ids)
        .execute(&mut **transaction)
        .await?;
    }
    sqlx::query(
        "UPDATE event_digests
         SET status = 'pending', message = NULL,
             verification_status = NULL, verification_label = NULL,
             verification_score = NULL, verification_reason = NULL,
             error = '事件聚类或来源设置已更新，等待重新核验',
             completed_at = NULL,
             agent_state = '{\"phase\":\"pending\",\"steps\":[\"source_invalidated\"]}'::jsonb,
             updated_at = clock_timestamp()
         WHERE story_id = ANY($1)",
    )
    .bind(story_ids)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub fn hash_content(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.trim().as_bytes());
    hex::encode(hasher.finalize())
}

pub fn detect_language(title: &str, content: &str) -> Option<String> {
    let sample = title.chars().chain(content.chars()).take(1_000);
    let mut letters = 0usize;
    let mut cjk = 0usize;
    for character in sample {
        if character.is_alphabetic() {
            letters += 1;
        }
        if matches!(character as u32, 0x3400..=0x9fff | 0xf900..=0xfaff) {
            cjk += 1;
        }
    }
    if letters == 0 {
        None
    } else if cjk * 5 >= letters {
        Some("zh".to_string())
    } else {
        Some("en".to_string())
    }
}

pub fn article_quality_score(
    content: &str,
    author: Option<&str>,
    extraction_error: Option<&str>,
    published_at: Option<DateTime<Utc>>,
) -> i32 {
    let length = content.chars().count();
    article_quality_score_from_signals(
        length,
        author.is_some_and(|value| !value.trim().is_empty()),
        extraction_error.is_none(),
        published_at.is_some(),
        content_noise_penalty(content),
    )
}

async fn load_identity_candidates(
    transaction: &mut Transaction<'_, Postgres>,
    cutoff: DateTime<Utc>,
    reference_time: DateTime<Utc>,
    source_family: &str,
    identity_tokens: &[String],
    entity_aliases: &[String],
) -> AppResult<Vec<sqlx::postgres::PgRow>> {
    if identity_tokens.len() < 2 && entity_aliases.is_empty() {
        return Ok(Vec::new());
    }

    let mut query = QueryBuilder::<Postgres>::new(
        "SELECT sc.id, a.title AS candidate_title, sc.story_key,
                sc.source_count AS candidate_source_count,
                sc.quality_score AS candidate_quality,
                EXISTS (
                  SELECT 1 FROM articles quality_article
                  JOIN feeds quality_feed ON quality_feed.id = quality_article.feed_id
                  WHERE quality_article.story_id = sc.id
                    AND quality_feed.quality_score >= 70
                ) AS candidate_has_wire_source,
                CASE
                  WHEN candidate_feed.hub_adapter IS NOT NULL AND candidate_feed.hub_path IS NOT NULL
                    THEN lower(candidate_feed.hub_adapter || ':' || candidate_feed.hub_path)
                  ELSE lower(regexp_replace(
                    COALESCE(
                      substring(COALESCE(NULLIF(candidate_feed.site_url, ''), candidate_feed.feed_url)
                        from '^[a-zA-Z][a-zA-Z0-9+.-]*://([^/:?#]+)'),
                      COALESCE(NULLIF(candidate_feed.site_url, ''), candidate_feed.feed_url)
                    ),
                    '^www[.]', ''
                  ))
                END = ",
    );
    query.push_bind(source_family.to_string());
    query.push(
        " AS same_source,
                COALESCE(a.published_at, a.fetched_at) AS last_reference_at
         FROM articles a
         JOIN story_clusters sc ON sc.id = a.story_id
         JOIN feeds candidate_feed ON candidate_feed.id = a.feed_id
         WHERE COALESCE(a.published_at, a.fetched_at) >= ",
    );
    query.push_bind(cutoff);
    query.push(" AND COALESCE(a.published_at, a.fetched_at) <= ");
    query.push_bind(reference_time);
    query.push(" AND (");

    let mut has_condition = false;
    for alias in entity_aliases {
        if has_condition {
            query.push(" OR ");
        }
        query.push("a.title ILIKE ");
        query.push_bind(format!("%{alias}%"));
        has_condition = true;
    }
    for left in 0..identity_tokens.len() {
        for right in left + 1..identity_tokens.len() {
            if has_condition {
                query.push(" OR ");
            }
            query.push("(a.title ILIKE ");
            query.push_bind(format!("%{}%", identity_tokens[left]));
            query.push(" AND a.title ILIKE ");
            query.push_bind(format!("%{}%", identity_tokens[right]));
            query.push(")");
            has_condition = true;
        }
    }
    query.push(") ORDER BY COALESCE(a.published_at, a.fetched_at) DESC LIMIT 500");

    Ok(query.build().fetch_all(&mut **transaction).await?)
}

fn article_quality_score_from_signals(
    length: usize,
    has_author: bool,
    extraction_succeeded: bool,
    has_published_at: bool,
    noise_penalty: i32,
) -> i32 {
    let mut score = 35;
    score += match length {
        0..=79 => 0,
        80..=299 => 8,
        300..=999 => 18,
        _ => 25,
    };
    if has_author {
        score += 8;
    }
    if has_published_at {
        score += 8;
    }
    if extraction_succeeded {
        score += 12;
    }
    score -= noise_penalty;
    score.clamp(0, 100)
}

fn content_noise_penalty(content: &str) -> i32 {
    let lower = content.to_ascii_lowercase();
    let mut penalty = 0;
    if [
        "googleanalyticsobject",
        "enable advertisement tracking and audience measurement",
        "browser extensions seems to be blocking",
        "checking your browser before accessing",
        "cf-chl-",
        "captcha",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        penalty = penalty.max(45);
    }
    if [
        "<script",
        "<style",
        "<iframe",
        "<div class=",
        "<!doctype",
        "</html>",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        penalty = penalty.max(35);
    }
    let json_pairs = content.matches("\":").count();
    let json_braces = content
        .chars()
        .filter(|character| matches!(character, '{' | '}' | '[' | ']'))
        .count();
    if json_pairs >= 8 && json_braces >= 2 {
        penalty = penalty.max(40);
    }
    penalty
}

pub async fn assign_story_cluster(state: &AppState, article_id: Uuid) -> AppResult<Uuid> {
    let mut transaction = state.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(STORY_ASSIGNMENT_LOCK_KEY)
        .fetch_one(&mut *transaction)
        .await?;
    let article = sqlx::query(
        "SELECT a.story_id, a.feed_id, a.title, a.published_at,
                f.site_url, f.feed_url, f.hub_adapter, f.hub_path
         FROM articles a
         JOIN feeds f ON f.id = a.feed_id
         WHERE a.id = $1
         FOR UPDATE OF a",
    )
    .bind(article_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::NotFound)?;
    let old_story_id = article.get::<Option<Uuid>, _>("story_id");
    let source_family = publisher_family(
        article.get::<Option<String>, _>("site_url").as_deref(),
        &article.get::<String, _>("feed_url"),
        article.get::<Option<String>, _>("hub_adapter").as_deref(),
        article.get::<Option<String>, _>("hub_path").as_deref(),
    );
    let title = article.get::<String, _>("title");
    let published_at = article.get::<Option<DateTime<Utc>>, _>("published_at");
    let tokens = story_tokens(&title);
    let has_semantic_action = tokens.iter().any(|token| token.starts_with("__action_"));
    let identity_tokens = if has_semantic_action {
        tokens
            .iter()
            .filter(|token| is_cross_language_identity_token(token))
            .take(8)
            .cloned()
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let entity_aliases = if has_semantic_action {
        story_entity_aliases(&tokens)
    } else {
        Vec::new()
    };
    let story_key = story_key(&tokens);
    let reference_time = published_at.unwrap_or_else(Utc::now);
    let cutoff = reference_time - STORY_WINDOW;
    sqlx::query("SET LOCAL pg_trgm.similarity_threshold = 0.12")
        .execute(&mut *transaction)
        .await?;
    let mut candidates = sqlx::query(
        "WITH candidates AS (
           SELECT sc.id, sc.representative_title AS candidate_title, sc.story_key,
                  sc.source_count AS candidate_source_count,
                  sc.quality_score AS candidate_quality,
                  EXISTS (
                    SELECT 1 FROM articles quality_article
                    JOIN feeds quality_feed ON quality_feed.id = quality_article.feed_id
                    WHERE quality_article.story_id = sc.id
                      AND quality_feed.quality_score >= 70
                  ) AS candidate_has_wire_source,
                  2.0::real AS rank,
                  EXISTS (
                    SELECT 1
                    FROM articles existing
                    JOIN feeds existing_feed ON existing_feed.id = existing.feed_id
                    WHERE existing.story_id = sc.id
                      AND CASE
                            WHEN existing_feed.hub_adapter IS NOT NULL AND existing_feed.hub_path IS NOT NULL
                              THEN lower(existing_feed.hub_adapter || ':' || existing_feed.hub_path)
                            ELSE lower(regexp_replace(
                              COALESCE(
                                substring(COALESCE(NULLIF(existing_feed.site_url, ''), existing_feed.feed_url)
                                  from '^[a-zA-Z][a-zA-Z0-9+.-]*://([^/:?#]+)'),
                                COALESCE(NULLIF(existing_feed.site_url, ''), existing_feed.feed_url)
                              ),
                              '^www[.]', ''
                            ))
                          END = $4
                  ) AS same_source,
                  COALESCE(sc.last_published_at, sc.updated_at) AS last_reference_at
           FROM story_clusters sc
           WHERE story_key = $2
             AND (last_published_at IS NULL OR last_published_at >= $1)
             AND (last_published_at IS NULL OR last_published_at <= $5)
           ORDER BY last_published_at DESC NULLS LAST, updated_at DESC
           LIMIT 20
         ), similar_candidates AS (
           SELECT sc.id, a.title AS candidate_title, sc.story_key,
                  sc.source_count AS candidate_source_count,
                  sc.quality_score AS candidate_quality,
                  EXISTS (
                    SELECT 1 FROM articles quality_article
                    JOIN feeds quality_feed ON quality_feed.id = quality_article.feed_id
                    WHERE quality_article.story_id = sc.id
                      AND quality_feed.quality_score >= 70
                  ) AS candidate_has_wire_source,
                  similarity(a.title, $3)::real AS rank,
                  CASE
                    WHEN candidate_feed.hub_adapter IS NOT NULL AND candidate_feed.hub_path IS NOT NULL
                      THEN lower(candidate_feed.hub_adapter || ':' || candidate_feed.hub_path)
                    ELSE lower(regexp_replace(
                      COALESCE(
                        substring(COALESCE(NULLIF(candidate_feed.site_url, ''), candidate_feed.feed_url)
                          from '^[a-zA-Z][a-zA-Z0-9+.-]*://([^/:?#]+)'),
                        COALESCE(NULLIF(candidate_feed.site_url, ''), candidate_feed.feed_url)
                      ),
                      '^www[.]', ''
                    ))
                  END = $4 AS same_source,
                  COALESCE(a.published_at, a.fetched_at) AS last_reference_at
           FROM articles a
           JOIN story_clusters sc ON sc.id = a.story_id
           JOIN feeds candidate_feed ON candidate_feed.id = a.feed_id
           WHERE COALESCE(a.published_at, a.fetched_at) >= $1
             AND COALESCE(a.published_at, a.fetched_at) <= $5
             AND a.title % $3
           ORDER BY similarity(a.title, $3) DESC
           LIMIT 500
         )
         SELECT id, candidate_title, story_key,
                candidate_source_count, candidate_quality,
                candidate_has_wire_source,
                same_source, last_reference_at
         FROM (
           SELECT * FROM candidates
           UNION ALL
           SELECT * FROM similar_candidates
         ) ranked
         ORDER BY rank DESC
         LIMIT 520",
    )
    .bind(cutoff)
    .bind(&story_key)
    .bind(&title)
    .bind(&source_family)
    .bind(reference_time)
    .fetch_all(&mut *transaction)
    .await?;
    candidates.extend(
        load_identity_candidates(
            &mut transaction,
            cutoff,
            reference_time,
            &source_family,
            &identity_tokens,
            &entity_aliases,
        )
        .await?,
    );

    let mut best: Option<(Uuid, f32, bool, i32, i32)> = None;
    let mut exact_matches = HashSet::new();
    for row in candidates {
        let candidate_id = row.get::<Uuid, _>("id");
        let candidate_key = row.get::<String, _>("story_key");
        let score = online_story_match_score(
            &title,
            row.get("candidate_title"),
            candidate_key == story_key,
            row.get("same_source"),
            reference_time - row.get::<DateTime<Utc>, _>("last_reference_at"),
        );
        if score >= 1.0 {
            exact_matches.insert(candidate_id);
        }
        let source_count = row.get::<i32, _>("candidate_source_count");
        let quality = row.get::<i32, _>("candidate_quality");
        let has_wire_source = row.get::<bool, _>("candidate_has_wire_source");
        if best.is_none_or(
            |(_, current_score, current_wire_source, current_sources, current_quality)| {
                score > current_score
                    || (score == current_score
                        && (has_wire_source, source_count, quality)
                            > (current_wire_source, current_sources, current_quality))
            },
        ) {
            best = Some((candidate_id, score, has_wire_source, source_count, quality));
        }
    }
    let (story_id, is_new_story) = if let Some((id, score, _, _, _)) =
        best.filter(|(_, score, _, _, _)| *score >= 1.0)
    {
        tracing::debug!(article_id = %article_id, story_id = %id, similarity = score, "article joined story cluster");
        (id, false)
    } else {
        let row = sqlx::query(
            "INSERT INTO story_clusters
               (representative_title, story_key, first_published_at, last_published_at)
             VALUES ($1, $2, $3, $3)
             RETURNING id",
        )
        .bind(&title)
        .bind(&story_key)
        .bind(published_at)
        .fetch_one(&mut *transaction)
        .await?;
        (row.get("id"), true)
    };

    if !is_new_story {
        let merged_story_ids = exact_matches
            .into_iter()
            .filter(|candidate_id| *candidate_id != story_id)
            .collect::<Vec<_>>();
        merge_story_clusters(&mut transaction, story_id, &merged_story_ids).await?;
    }

    sqlx::query(
        "UPDATE articles
         SET story_id = $2, needs_recluster = false, updated_at = now()
         WHERE id = $1",
    )
    .bind(article_id)
    .bind(story_id)
    .execute(&mut *transaction)
    .await?;
    let previous_story_id = old_story_id.filter(|old_story_id| *old_story_id != story_id);
    if let Some(old_story_id) = previous_story_id {
        refresh_story_cluster(&mut transaction, old_story_id).await?;
    }
    refresh_story_cluster(&mut transaction, story_id).await?;
    crate::world::record_event_revision(
        &mut transaction,
        story_id,
        article_id,
        &title,
        published_at.unwrap_or(reference_time),
        is_new_story,
    )
    .await?;
    if let Some(old_story_id) = previous_story_id {
        invalidate_event_digest_for_story(&mut transaction, old_story_id).await?;
    }
    invalidate_event_digest_for_story(&mut transaction, story_id).await?;
    transaction.commit().await?;
    Ok(story_id)
}

async fn merge_story_clusters(
    transaction: &mut Transaction<'_, Postgres>,
    winner: Uuid,
    merged_story_ids: &[Uuid],
) -> AppResult<()> {
    if merged_story_ids.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO event_feedback (story_id, kind, created_at, updated_at)
         SELECT $1, kind, created_at, updated_at
         FROM event_feedback
         WHERE story_id = ANY($2)
         ORDER BY updated_at DESC
         LIMIT 1
         ON CONFLICT (story_id) DO NOTHING",
    )
    .bind(winner)
    .bind(merged_story_ids)
    .execute(&mut **transaction)
    .await?;
    sqlx::query("DELETE FROM event_feedback WHERE story_id = ANY($1)")
        .bind(merged_story_ids)
        .execute(&mut **transaction)
        .await?;
    sqlx::query("UPDATE event_revisions SET story_id = $1 WHERE story_id = ANY($2)")
        .bind(winner)
        .bind(merged_story_ids)
        .execute(&mut **transaction)
        .await?;
    sqlx::query(
        "UPDATE articles
         SET story_id = $1, needs_recluster = false, updated_at = now()
         WHERE story_id = ANY($2)",
    )
    .bind(winner)
    .bind(merged_story_ids)
    .execute(&mut **transaction)
    .await?;
    sqlx::query("DELETE FROM story_clusters WHERE id = ANY($1)")
        .bind(merged_story_ids)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

struct ClusterDraft {
    id: Uuid,
    representative_title: String,
    story_key: String,
    story_keys: HashSet<String>,
    canonical_urls: HashSet<String>,
    title_variants: Vec<(BTreeSet<String>, BTreeSet<String>)>,
    last_reference_at: DateTime<Utc>,
    source_keys: HashSet<String>,
    quality_total: i64,
    article_count: i64,
    first_published_at: Option<DateTime<Utc>>,
    last_published_at: Option<DateTime<Utc>>,
}

struct ArticleAssignment {
    id: Uuid,
    language: String,
    quality_score: i32,
    content_hash: String,
    story_id: Uuid,
}

pub async fn rebuild_all(state: &AppState) -> AppResult<i64> {
    let mut transaction = state.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(STORY_ASSIGNMENT_LOCK_KEY)
        .fetch_one(&mut *transaction)
        .await?;
    let rows = sqlx::query(
        "SELECT a.id, a.feed_id, a.title, a.author, a.extraction_error,
                a.canonical_url, a.published_at, a.fetched_at,
                f.site_url, f.feed_url, f.hub_adapter, f.hub_path,
                f.quality_score AS feed_quality_score,
                LEFT(a.title || ' ' || COALESCE(NULLIF(a.extracted_content, ''), a.content), 1000) AS language_sample,
                LEFT(COALESCE(NULLIF(a.extracted_content, ''), a.content), 4000) AS content_sample,
                char_length(COALESCE(NULLIF(a.extracted_content, ''), a.content))::bigint AS content_length,
                encode(digest(btrim(COALESCE(NULLIF(a.extracted_content, ''), a.content)), 'sha256'), 'hex') AS content_hash
         FROM articles a
         JOIN feeds f ON f.id = a.feed_id
         ORDER BY COALESCE(a.published_at, a.fetched_at), a.id",
    )
    .fetch_all(&mut *transaction)
    .await?;
    let count = rows.len() as i64;
    let mut clusters = Vec::<ClusterDraft>::new();
    let mut token_index = HashMap::<String, Vec<usize>>::new();
    let mut key_index = HashMap::<String, Vec<usize>>::new();
    let mut url_index = HashMap::<String, usize>::new();
    let mut assignments = Vec::<ArticleAssignment>::with_capacity(rows.len());

    for row in rows {
        let article_id = row.get("id");
        let title = row.get::<String, _>("title");
        let canonical_url = row.get::<String, _>("canonical_url");
        let site_url = row.get::<Option<String>, _>("site_url");
        let feed_url = row.get::<String, _>("feed_url");
        let hub_adapter = row.get::<Option<String>, _>("hub_adapter");
        let hub_path = row.get::<Option<String>, _>("hub_path");
        let source_key = publisher_family(
            site_url.as_deref(),
            &feed_url,
            hub_adapter.as_deref(),
            hub_path.as_deref(),
        );
        let published_at = row.get::<Option<DateTime<Utc>>, _>("published_at");
        let reference_at = published_at.unwrap_or_else(|| row.get("fetched_at"));
        let cutoff = reference_at - STORY_WINDOW;

        let tokens = story_tokens(&title);
        let bigrams = character_bigrams(&title);
        let key = story_key(&tokens);
        let mut best = None::<(usize, f32)>;
        let mut candidate_indices = HashSet::new();
        if let Some(index) = url_index.get(&canonical_url).copied()
            && clusters[index].last_reference_at >= cutoff
        {
            candidate_indices.insert(index);
        }
        if let Some(indices) = key_index.get(&key)
            && let Some(index) = indices
                .iter()
                .rev()
                .copied()
                .find(|index| clusters[*index].last_reference_at >= cutoff)
        {
            candidate_indices.insert(index);
        }
        for token in &tokens {
            if let Some(indices) = token_index.get(token) {
                for index in indices
                    .iter()
                    .rev()
                    .copied()
                    .filter(|index| clusters[*index].last_reference_at >= cutoff)
                    .take(500)
                {
                    if candidate_indices.len() >= MAX_APPROXIMATE_CANDIDATES {
                        break;
                    }
                    candidate_indices.insert(index);
                }
            }
        }
        for index in candidate_indices {
            let cluster = &clusters[index];
            let score = cluster
                .title_variants
                .iter()
                .map(|(candidate_tokens, candidate_bigrams)| {
                    story_match_score(
                        &tokens,
                        &bigrams,
                        candidate_tokens,
                        candidate_bigrams,
                        StoryMatchContext {
                            same_key: cluster.story_keys.contains(&key),
                            same_url: cluster.canonical_urls.contains(&canonical_url),
                            same_source: cluster.source_keys.contains(&source_key),
                            elapsed: reference_at - cluster.last_reference_at,
                        },
                    )
                })
                .fold(0.0, f32::max);
            if score > best.map(|(_, current)| current).unwrap_or_default() {
                best = Some((index, score));
            }
        }
        let cluster_index = if let Some((index, _)) = best.filter(|(_, score)| *score >= 1.0) {
            index
        } else {
            let index = clusters.len();
            clusters.push(ClusterDraft {
                id: Uuid::new_v4(),
                representative_title: title.clone(),
                story_key: key.clone(),
                story_keys: HashSet::new(),
                canonical_urls: HashSet::new(),
                title_variants: Vec::new(),
                last_reference_at: reference_at,
                source_keys: HashSet::new(),
                quality_total: 0,
                article_count: 0,
                first_published_at: published_at,
                last_published_at: published_at,
            });
            index
        };

        let cluster = &mut clusters[cluster_index];
        if cluster.story_keys.insert(key.clone()) {
            key_index.entry(key).or_default().push(cluster_index);
            for token in &tokens {
                token_index
                    .entry(token.clone())
                    .or_default()
                    .push(cluster_index);
            }
            if cluster.title_variants.len() >= 8 {
                cluster.title_variants.remove(1);
            }
            cluster
                .title_variants
                .push((tokens.clone(), bigrams.clone()));
        }
        if cluster.canonical_urls.insert(canonical_url.clone()) {
            url_index.insert(canonical_url, cluster_index);
        }

        let content_length = row.get::<i64, _>("content_length").max(0) as usize;
        let author = row.get::<Option<String>, _>("author");
        let content_sample = row.get::<String, _>("content_sample");
        let quality_score = article_quality_score_from_signals(
            content_length,
            author.is_some_and(|value| !value.trim().is_empty()),
            row.get::<Option<String>, _>("extraction_error").is_none(),
            published_at.is_some(),
            content_noise_penalty(&content_sample),
        );
        cluster.last_reference_at = reference_at;
        cluster.source_keys.insert(source_key);
        cluster.quality_total +=
            i64::from((quality_score + row.get::<i32, _>("feed_quality_score")) / 2);
        cluster.article_count += 1;
        if let Some(published_at) = published_at {
            cluster.first_published_at = Some(
                cluster
                    .first_published_at
                    .map_or(published_at, |current| current.min(published_at)),
            );
            cluster.last_published_at = Some(
                cluster
                    .last_published_at
                    .map_or(published_at, |current| current.max(published_at)),
            );
        }
        let language_sample = row.get::<String, _>("language_sample");
        assignments.push(ArticleAssignment {
            id: article_id,
            language: detect_language("", &language_sample).unwrap_or_else(|| "und".to_string()),
            quality_score,
            content_hash: row.get("content_hash"),
            story_id: cluster.id,
        });
    }

    sqlx::query("UPDATE articles SET story_id = NULL")
        .execute(&mut *transaction)
        .await?;
    sqlx::query("DELETE FROM story_clusters")
        .execute(&mut *transaction)
        .await?;
    for chunk in clusters.chunks(1_000) {
        let mut builder = QueryBuilder::<Postgres>::new(
            "INSERT INTO story_clusters
               (id, representative_title, story_key, source_count, quality_score,
                first_published_at, last_published_at) ",
        );
        builder.push_values(chunk, |mut row, cluster| {
            let quality_score = (cluster.quality_total / cluster.article_count.max(1)) as i32;
            row.push_bind(cluster.id)
                .push_bind(&cluster.representative_title)
                .push_bind(&cluster.story_key)
                .push_bind(cluster.source_keys.len() as i32)
                .push_bind(quality_score.clamp(0, 100))
                .push_bind(cluster.first_published_at)
                .push_bind(cluster.last_published_at);
        });
        builder.build().execute(&mut *transaction).await?;
    }
    for chunk in assignments.chunks(5_000) {
        let ids = chunk.iter().map(|item| item.id).collect::<Vec<_>>();
        let languages = chunk
            .iter()
            .map(|item| item.language.clone())
            .collect::<Vec<_>>();
        let quality_scores = chunk
            .iter()
            .map(|item| item.quality_score)
            .collect::<Vec<_>>();
        let content_hashes = chunk
            .iter()
            .map(|item| item.content_hash.clone())
            .collect::<Vec<_>>();
        let story_ids = chunk.iter().map(|item| item.story_id).collect::<Vec<_>>();
        sqlx::query(
            "UPDATE articles a
             SET language = metadata.language,
                 quality_score = metadata.quality_score,
                 content_hash = metadata.content_hash,
                 story_id = metadata.story_id,
                 needs_recluster = false,
                 updated_at = now()
             FROM UNNEST($1::uuid[], $2::text[], $3::int4[], $4::text[], $5::uuid[])
               AS metadata(id, language, quality_score, content_hash, story_id)
             WHERE a.id = metadata.id",
        )
        .bind(ids)
        .bind(languages)
        .bind(quality_scores)
        .bind(content_hashes)
        .bind(story_ids)
        .execute(&mut *transaction)
        .await?;
    }
    seed_event_revisions(&mut transaction).await?;
    transaction.commit().await?;
    Ok(count)
}

pub async fn recluster_recent(
    state: &AppState,
    hours: i64,
    min_feed_quality: i32,
) -> AppResult<i64> {
    let hours = hours.clamp(1, 24 * 14);
    let min_feed_quality = min_feed_quality.clamp(0, 100);
    let mut transaction = state.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(STORY_ASSIGNMENT_LOCK_KEY)
        .fetch_one(&mut *transaction)
        .await?;
    let rows = sqlx::query(
        "SELECT a.id, a.story_id
         FROM articles a
         JOIN feeds f ON f.id = a.feed_id
         WHERE COALESCE(a.published_at, a.fetched_at)
                 >= now() - make_interval(hours => $1::integer)
           AND f.quality_score >= $2
         ORDER BY COALESCE(a.published_at, a.fetched_at), a.id",
    )
    .bind(hours as i32)
    .bind(min_feed_quality)
    .fetch_all(&mut *transaction)
    .await?;
    let article_ids = rows
        .iter()
        .map(|row| row.get::<Uuid, _>("id"))
        .collect::<Vec<_>>();
    if article_ids.is_empty() {
        transaction.commit().await?;
        return Ok(0);
    }
    let affected_story_ids = rows
        .iter()
        .filter_map(|row| row.get::<Option<Uuid>, _>("story_id"))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    sqlx::query(
        "UPDATE articles
         SET story_id = NULL, needs_recluster = true, updated_at = clock_timestamp()
         WHERE id = ANY($1)",
    )
    .bind(&article_ids)
    .execute(&mut *transaction)
    .await?;
    refresh_story_clusters_after_source_change(&mut transaction, &affected_story_ids, true).await?;
    transaction.commit().await?;

    let mut clustered = 0i64;
    for article_id in article_ids {
        match assign_story_cluster(state, article_id).await {
            Ok(_) => clustered += 1,
            Err(error) => {
                tracing::warn!(%article_id, %error, "recent article recluster failed");
            }
        }
    }
    Ok(clustered)
}

pub async fn recluster_article(state: &AppState, article_id: Uuid) -> AppResult<Uuid> {
    let mut transaction = state.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(STORY_ASSIGNMENT_LOCK_KEY)
        .fetch_one(&mut *transaction)
        .await?;
    let old_story_id = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT story_id FROM articles WHERE id = $1 FOR UPDATE",
    )
    .bind(article_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::NotFound)?;
    sqlx::query(
        "UPDATE articles
         SET story_id = NULL, needs_recluster = true, updated_at = clock_timestamp()
         WHERE id = $1",
    )
    .bind(article_id)
    .execute(&mut *transaction)
    .await?;
    if let Some(old_story_id) = old_story_id {
        refresh_story_clusters_after_source_change(&mut transaction, &[old_story_id], true).await?;
    }
    transaction.commit().await?;
    assign_story_cluster(state, article_id).await
}

async fn refresh_story_cluster(
    transaction: &mut Transaction<'_, Postgres>,
    story_id: Uuid,
) -> AppResult<()> {
    let updated = sqlx::query(
        "UPDATE story_clusters sc
         SET source_count = stats.source_count,
             quality_score = stats.quality_score,
             first_published_at = stats.first_published_at,
             last_published_at = stats.last_published_at,
             updated_at = now()
         FROM (
           SELECT COUNT(DISTINCT CASE
                    WHEN f.hub_adapter IS NOT NULL AND f.hub_path IS NOT NULL
                      THEN lower(f.hub_adapter || ':' || f.hub_path)
                    ELSE lower(regexp_replace(
                      COALESCE(
                        substring(COALESCE(NULLIF(f.site_url, ''), f.feed_url)
                          from '^[a-zA-Z][a-zA-Z0-9+.-]*://([^/:?#]+)'),
                        COALESCE(NULLIF(f.site_url, ''), f.feed_url)
                      ),
                      '^www[.]', ''
                    ))
                  END)::integer AS source_count,
                  ROUND(AVG((a.quality_score + f.quality_score)::numeric / 2))::integer AS quality_score,
                  MIN(a.published_at) AS first_published_at,
                  MAX(a.published_at) AS last_published_at
           FROM articles a
           JOIN feeds f ON f.id = a.feed_id
           WHERE a.story_id = $1
           HAVING COUNT(*) > 0
         ) stats
         WHERE sc.id = $1",
    )
    .bind(story_id)
    .execute(&mut **transaction)
    .await?
    .rows_affected();
    if updated == 0 {
        sqlx::query(
            "DELETE FROM story_clusters sc
             WHERE sc.id = $1
               AND NOT EXISTS (SELECT 1 FROM articles a WHERE a.story_id = sc.id)
               AND NOT EXISTS (SELECT 1 FROM event_feedback ef WHERE ef.story_id = sc.id)",
        )
        .bind(story_id)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}

async fn invalidate_event_digest_for_story(
    transaction: &mut Transaction<'_, Postgres>,
    story_id: Uuid,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE event_digests
         SET status = 'pending', message = NULL,
             verification_status = NULL, verification_label = NULL,
             verification_score = NULL, verification_reason = NULL,
             error = '事件聚类或修订已更新，等待重新核验',
             completed_at = NULL,
             agent_state = '{\"phase\":\"pending\",\"steps\":[\"story_invalidated\"]}'::jsonb,
             updated_at = clock_timestamp()
         WHERE story_id = $1",
    )
    .bind(story_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn seed_event_revisions(transaction: &mut Transaction<'_, Postgres>) -> AppResult<()> {
    sqlx::query(
        "WITH observations AS (
           SELECT a.story_id,
                  a.id AS article_id,
                  a.title,
                  GREATEST((
                    SELECT COUNT(DISTINCT CASE
                      WHEN prior_feed.hub_adapter IS NOT NULL AND prior_feed.hub_path IS NOT NULL
                        THEN lower(prior_feed.hub_adapter || ':' || prior_feed.hub_path)
                      ELSE lower(regexp_replace(
                        COALESCE(
                          substring(COALESCE(NULLIF(prior_feed.site_url, ''), prior_feed.feed_url)
                            from '^[a-zA-Z][a-zA-Z0-9+.-]*://([^/:?#]+)'),
                          COALESCE(NULLIF(prior_feed.site_url, ''), prior_feed.feed_url)
                        ),
                        '^www[.]', ''
                      ))
                    END)
                    FROM articles prior
                    JOIN feeds prior_feed ON prior_feed.id = prior.feed_id
                    WHERE prior.story_id = a.story_id
                      AND COALESCE(prior.published_at, prior.fetched_at)
                          <= COALESCE(a.published_at, a.fetched_at)
                  ), 1)::integer AS source_count,
                  COALESCE(a.published_at, a.fetched_at) AS occurred_at,
                  ROW_NUMBER() OVER (
                    PARTITION BY a.story_id
                    ORDER BY COALESCE(a.published_at, a.fetched_at), a.id
                  ) AS position
           FROM articles a
           JOIN story_clusters sc ON sc.id = a.story_id
           WHERE COALESCE(a.published_at, a.fetched_at) >= now() - interval '30 days'
         )
         INSERT INTO event_revisions
           (story_id, article_id, revision_type, title, source_count, occurred_at)
         SELECT story_id,
                article_id,
                CASE
                  WHEN lower(title) ~ '(correction|corrected|修正|更正|纠正)' THEN 'CORRECT'
                  WHEN lower(title) ~ '(fact[ -]?check|false (claim|report)|(denies|denied) (the )?(claim|report|allegation)|refutes? (claim|report)|debunks? (claim|report)|官方否认|否认.{0,12}(报道|指控|说法|传闻)|辟谣|(报道|消息|说法|传闻)不实)' THEN 'CONTRADICT'
                  WHEN position = 1 THEN 'ADD'
                  WHEN source_count >= 2 THEN 'CONFIRM'
                  ELSE 'UPDATE'
                END,
                title,
                source_count,
                occurred_at
         FROM observations
         ON CONFLICT (article_id) WHERE article_id IS NOT NULL DO NOTHING",
    )
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn story_key(tokens: &BTreeSet<String>) -> String {
    let mut hasher = Sha256::new();
    for token in tokens {
        hasher.update(token.as_bytes());
        hasher.update([0]);
    }
    hex::encode(&hasher.finalize()[..16])
}

pub(crate) fn publisher_family(
    site_url: Option<&str>,
    feed_url: &str,
    hub_adapter: Option<&str>,
    hub_path: Option<&str>,
) -> String {
    if let (Some(adapter), Some(path)) = (hub_adapter, hub_path)
        && !adapter.trim().is_empty()
        && !path.trim().is_empty()
    {
        return format!("{}:{}", adapter.trim(), path.trim()).to_lowercase();
    }
    let value = site_url
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(feed_url);
    reqwest::Url::parse(value)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .map(|host| host.trim_start_matches("www.").to_string())
        .unwrap_or_else(|| value.to_lowercase())
}

fn story_tokens(title: &str) -> BTreeSet<String> {
    let normalized = title.to_lowercase();
    let mut tokens = BTreeSet::new();
    for segment in normalized.split(|character: char| !character.is_alphanumeric()) {
        let mut run = String::new();
        let mut run_is_ascii = None;
        for character in segment.chars() {
            let is_ascii = character.is_ascii();
            if run_is_ascii.is_some_and(|current| current != is_ascii) {
                insert_story_token_run(&mut tokens, &run, run_is_ascii.unwrap_or(false));
                run.clear();
            }
            run_is_ascii = Some(is_ascii);
            run.push(character);
        }
        insert_story_token_run(&mut tokens, &run, run_is_ascii.unwrap_or(false));
    }
    insert_story_semantic_markers(&normalized, &mut tokens);
    tokens
}

fn insert_story_semantic_markers(normalized: &str, tokens: &mut BTreeSet<String>) {
    let compact = normalized
        .chars()
        .filter(|character| character.is_alphanumeric())
        .collect::<String>();
    if normalized.contains("apple intelligence") || compact.contains("苹果智能") {
        tokens.insert("__entity_apple_intelligence".to_string());
    }
    if normalized.contains("apple music") {
        tokens.insert("__entity_apple_music".to_string());
    }
    if normalized.contains("iphone") {
        tokens.insert("__entity_iphone".to_string());
    }
    if normalized.contains("icloud") {
        tokens.insert("__entity_icloud".to_string());
    }
    if normalized.contains("apple music")
        || normalized.contains("icloud")
        || normalized.contains("apple one")
    {
        tokens.insert("__entity_apple_services".to_string());
    }
    if compact.contains("中国") || compact.contains("在华") {
        tokens.insert("__geo_cn".to_string());
    }
    if compact.contains("日本") {
        tokens.insert("__geo_jp".to_string());
    }
    if normalized.contains("kimi k3")
        || compact.contains("kimik3")
        || normalized.contains("moonshot")
        || compact.contains("月之暗面")
    {
        tokens.insert("__entity_moonshot_kimi".to_string());
    }
    if normalized.contains("trump") || compact.contains("特朗普") || compact.contains("川普") {
        tokens.insert("__entity_trump".to_string());
    }
    if normalized.contains("truth social")
        || normalized.contains("trump media")
        || compact.contains("真相社交")
    {
        tokens.insert("__entity_truth_social".to_string());
    }
    let mentions_canada = normalized.contains("canada")
        || normalized.contains("canadian")
        || compact.contains("加拿大");
    let mentions_wildfire_smoke = ["wildfire", "smoke", "山火", "野火", "烟雾", "浓烟"]
        .iter()
        .any(|marker| normalized.contains(marker));
    if mentions_canada && mentions_wildfire_smoke {
        tokens.insert("__entity_canada_wildfire_smoke".to_string());
    }
    if normalized.contains("cleared")
        || normalized.contains("approved")
        || normalized.contains("registered")
        || normalized.contains("备案")
        || normalized.contains("获准")
        || normalized.contains("通过监管")
    {
        tokens.insert("__action_regulatory_approval".to_string());
    }
    if normalized.contains("delayed")
        || normalized.contains("postponed")
        || normalized.contains("推迟")
        || normalized.contains("延期")
    {
        tokens.insert("__action_delay".to_string());
    }
    if normalized.contains("anti-ai") || compact.contains("antiai") || compact.contains("反ai") {
        tokens.insert("__action_anti_ai_position".to_string());
    }
    if ["in talks", "negotiat", "lease computing", "洽谈", "谈判"]
        .iter()
        .any(|marker| normalized.contains(marker))
        || ((normalized.contains("出租") || normalized.contains("租用"))
            && normalized.contains("算力"))
    {
        tokens.insert("__action_negotiation".to_string());
    }
    if [
        "integration",
        "integrates",
        "integrated",
        "sign in",
        "log in",
        "集成",
        "整合",
        "代登录",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
    {
        tokens.insert("__action_integration".to_string());
    }
    let mentions_price_change = ((normalized.contains("raise")
        || normalized.contains("increase")
        || normalized.contains("adjust"))
        && (normalized.contains("price") || normalized.contains("cost")))
        || ((normalized.contains("上调") || normalized.contains("调整"))
            && (normalized.contains("价格")
                || normalized.contains("售价")
                || normalized.contains("订阅")))
        || ["price hike", "涨价", "上调价格", "上调售价", "上调订阅"]
            .iter()
            .any(|marker| normalized.contains(marker));
    if mentions_price_change {
        tokens.insert("__action_price_change".to_string());
    }
    if ((normalized.contains("raise") || normalized.contains("increase"))
        && (normalized.contains("price") || normalized.contains("cost")))
        || (normalized.contains("上调")
            && (normalized.contains("价格")
                || normalized.contains("售价")
                || normalized.contains("订阅")))
        || ["price hike", "涨价", "上调价格", "上调售价", "上调订阅"]
            .iter()
            .any(|marker| normalized.contains(marker))
    {
        tokens.insert("__action_price_increase".to_string());
    }
    if ["lawsuit", "sues", "sued", "legal action", "起诉", "诉讼"]
        .iter()
        .any(|marker| normalized.contains(marker))
    {
        tokens.insert("__action_litigation".to_string());
    }
    if normalized.contains("tariff") || normalized.contains("关税") || normalized.contains("加税")
    {
        tokens.insert("__action_tariff_change".to_string());
    }
    if (normalized.contains("tariff") && normalized.contains("threat"))
        || (normalized.contains("关税") && normalized.contains("威胁"))
        || normalized.contains("加税")
    {
        tokens.insert("__action_tariff_threat".to_string());
    }
    if normalized.contains("deepswe")
        && ["third", "top three", "rank", "前三", "第三", "排名", "首登"]
            .iter()
            .any(|marker| normalized.contains(marker))
    {
        tokens.insert("__action_benchmark_ranking".to_string());
    }
    if [
        "priority access",
        "early access",
        "high-speed access",
        "instant access",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
        || (normalized.contains("sell") && normalized.contains("access"))
        || (normalized.contains("出售") && normalized.contains("访问"))
    {
        tokens.insert("__action_priority_access".to_string());
    }
    if [
        "released", "releases", "unveils", "launched", "launches", "发布", "推出",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
    {
        tokens.insert("__action_release".to_string());
    }
    if [
        "extending access",
        "included in",
        "remain available",
        "保留",
        "纳入",
        "暂不下线",
        "上线",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
    {
        tokens.insert("__action_availability".to_string());
    }
}

fn insert_story_token_run(tokens: &mut BTreeSet<String>, run: &str, is_ascii: bool) {
    if run.is_empty() || is_stop_word(run) {
        return;
    }
    let characters = run.chars().collect::<Vec<_>>();
    if is_ascii {
        if characters.len() >= 2 || characters.iter().all(char::is_ascii_digit) {
            tokens.insert(run.to_string());
            let normalized = normalize_ascii_story_token(run);
            if normalized != run {
                tokens.insert(normalized);
            }
        }
    } else if characters.len() == 1 {
        tokens.insert(run.to_string());
    } else {
        for pair in characters.windows(2) {
            tokens.insert(pair.iter().collect());
        }
    }
}

fn normalize_ascii_story_token(value: &str) -> String {
    if value.len() <= 4 || value.chars().all(|character| character.is_ascii_digit()) {
        return value.to_string();
    }
    if matches!(value, "dies" | "lies" | "ties") {
        return value[..value.len() - 1].to_string();
    }
    if let Some(stem) = value.strip_suffix("ies") {
        return format!("{stem}y");
    }
    if let Some(stem) = value.strip_suffix("ied") {
        return format!("{stem}y");
    }
    if let Some(stem) = value.strip_suffix("ing")
        && stem.len() >= 4
    {
        return stem.to_string();
    }
    if let Some(stem) = value.strip_suffix("ed")
        && stem.len() >= 4
    {
        if stem.ends_with(['g', 'c', 'v', 's']) {
            return format!("{stem}e");
        }
        return stem.to_string();
    }
    if let Some(stem) = value.strip_suffix('s')
        && stem.len() >= 4
        && !value.ends_with("ss")
        && !value.ends_with("us")
        && !value.ends_with("is")
        && [
            "ions", "acks", "lks", "ges", "ces", "ses", "xes", "zes", "ens", "kes",
        ]
        .iter()
        .any(|suffix| value.ends_with(suffix))
    {
        return stem.to_string();
    }
    value.to_string()
}

fn is_stop_word(value: &str) -> bool {
    matches!(
        value,
        "the"
            | "a"
            | "an"
            | "and"
            | "or"
            | "of"
            | "to"
            | "in"
            | "for"
            | "on"
            | "with"
            | "as"
            | "at"
            | "by"
            | "from"
            | "is"
            | "are"
            | "was"
            | "were"
            | "be"
            | "been"
            | "before"
            | "after"
            | "over"
            | "against"
            | "more"
            | "than"
            | "this"
            | "that"
            | "his"
            | "her"
            | "its"
    )
}

fn jaccard(left: &BTreeSet<String>, right: &BTreeSet<String>) -> f32 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let intersection = left.intersection(right).count() as f32;
    let union = left.union(right).count() as f32;
    intersection / union
}

fn geography_markers(tokens: &BTreeSet<String>) -> HashSet<&'static str> {
    tokens
        .iter()
        .filter_map(|token| match token.as_str() {
            "uk" | "britain" | "british" | "england" | "english" => Some("gb"),
            "us" | "usa" | "america" | "american" => Some("us"),
            "canada" | "canadian" => Some("ca"),
            "china" | "chinese" => Some("cn"),
            "__geo_cn" => Some("cn"),
            "japan" | "japanese" | "__geo_jp" => Some("jp"),
            "iran" | "iranian" => Some("ir"),
            "israel" | "israeli" => Some("il"),
            "palestine" | "palestinian" | "gaza" => Some("ps"),
            "ukraine" | "ukrainian" => Some("ua"),
            "russia" | "russian" => Some("ru"),
            "france" | "french" => Some("fr"),
            "germany" | "german" => Some("de"),
            "spain" | "spanish" => Some("es"),
            "india" | "indian" => Some("in"),
            "australia" | "australian" => Some("au"),
            "venezuela" | "venezuelan" => Some("ve"),
            "nigeria" | "nigerian" => Some("ng"),
            _ => None,
        })
        .collect()
}

struct StoryMatchContext {
    same_key: bool,
    same_url: bool,
    same_source: bool,
    elapsed: Duration,
}

fn story_match_score(
    left_tokens: &BTreeSet<String>,
    left_bigrams: &BTreeSet<String>,
    right_tokens: &BTreeSet<String>,
    right_bigrams: &BTreeSet<String>,
    context: StoryMatchContext,
) -> f32 {
    if context.same_url {
        return 1.0;
    }
    if context.same_key {
        return if !context.same_source || context.elapsed <= SAME_SOURCE_WINDOW {
            1.0
        } else {
            0.0
        };
    }
    if left_tokens.is_empty() || right_tokens.is_empty() {
        return 0.0;
    }
    let intersection = left_tokens.intersection(right_tokens).count();
    let real_intersection = left_tokens
        .intersection(right_tokens)
        .filter(|token| !token.starts_with("__") && !is_generic_story_action_lexeme(token))
        .count();
    let min_tokens = left_tokens.len().min(right_tokens.len()).max(1) as f32;
    let containment = intersection as f32 / min_tokens;
    let token_score = jaccard(left_tokens, right_tokens);
    let dice = if left_bigrams.is_empty() || right_bigrams.is_empty() {
        0.0
    } else {
        let common = left_bigrams.intersection(right_bigrams).count() as f32;
        2.0 * common / (left_bigrams.len() + right_bigrams.len()) as f32
    };
    let bigram_intersection = left_bigrams.intersection(right_bigrams).count();
    let bigram_containment =
        bigram_intersection as f32 / left_bigrams.len().min(right_bigrams.len()).max(1) as f32;
    let left_has_non_ascii = left_tokens.iter().any(|token| !token.is_ascii());
    let right_has_non_ascii = right_tokens.iter().any(|token| !token.is_ascii());
    let shared_ascii_topics = left_tokens
        .intersection(right_tokens)
        .filter(|token| is_cross_language_identity_token(token))
        .count();
    let has_distinctive_ascii_topic = left_tokens
        .intersection(right_tokens)
        .any(|token| is_cross_language_identity_token(token) && token.len() >= 4);
    let shared_cross_language_identities = left_tokens
        .intersection(right_tokens)
        .filter(|token| is_cross_language_identity_token(token))
        .count();
    let has_shared_event_action = left_tokens
        .intersection(right_tokens)
        .any(|token| is_event_action_token(token) || token.starts_with("__action_"));
    let has_shared_semantic_entity = left_tokens
        .intersection(right_tokens)
        .any(|token| token.starts_with("__entity_"));
    let has_shared_scoped_semantic_entity = left_tokens
        .intersection(right_tokens)
        .any(|token| token.starts_with("__entity_") && token.as_str() != "__entity_trump");
    let has_shared_semantic_action = left_tokens
        .intersection(right_tokens)
        .any(|token| token.starts_with("__action_"));
    let left_has_semantic_action = left_tokens
        .iter()
        .any(|token| token.starts_with("__action_"));
    let right_has_semantic_action = right_tokens
        .iter()
        .any(|token| token.starts_with("__action_"));
    let left_semantic_entities = left_tokens
        .iter()
        .filter(|token| token.starts_with("__entity_"))
        .collect::<HashSet<_>>();
    let right_semantic_entities = right_tokens
        .iter()
        .filter(|token| token.starts_with("__entity_"))
        .collect::<HashSet<_>>();

    let left_geography = geography_markers(left_tokens);
    let right_geography = geography_markers(right_tokens);
    if !left_geography.is_empty()
        && !right_geography.is_empty()
        && left_geography.is_disjoint(&right_geography)
    {
        return token_score.max(dice).min(0.99);
    }
    if !left_semantic_entities.is_empty()
        && !right_semantic_entities.is_empty()
        && left_semantic_entities.is_disjoint(&right_semantic_entities)
    {
        return token_score.max(dice).min(0.99);
    }
    if has_shared_semantic_entity
        && left_has_semantic_action
        && right_has_semantic_action
        && !has_shared_semantic_action
    {
        return token_score.max(dice).min(0.99);
    }

    let matches = if context.same_source {
        false
    } else {
        (has_shared_scoped_semantic_entity && has_shared_semantic_action)
            || dice >= 0.82
            || (real_intersection >= 5
                && containment >= 0.60
                && token_score >= 0.35
                && dice >= 0.50)
            || (real_intersection >= 4
                && containment >= 0.50
                && dice >= 0.40
                && has_shared_event_action)
            || (real_intersection >= 3 && containment >= 0.70 && dice >= 0.62)
            || (left_has_non_ascii != right_has_non_ascii
                && shared_ascii_topics >= 4
                && has_distinctive_ascii_topic)
            || (shared_cross_language_identities >= 2
                && has_shared_semantic_action
                && context.elapsed >= -CROSS_LANGUAGE_WINDOW
                && context.elapsed <= CROSS_LANGUAGE_WINDOW)
            || (left_has_non_ascii
                && right_has_non_ascii
                && left_tokens.len().min(right_tokens.len()) <= 10
                && real_intersection >= 4
                && containment >= 0.50
                && bigram_intersection >= 4
                && bigram_containment >= 0.50
                && has_shared_event_action)
    };
    if matches {
        1.0
    } else {
        token_score.max(dice).min(0.99)
    }
}

fn is_generic_story_action_lexeme(token: &str) -> bool {
    matches!(
        token,
        "adjust"
            | "adjusted"
            | "adjusts"
            | "change"
            | "changed"
            | "changes"
            | "cost"
            | "costs"
            | "increase"
            | "increased"
            | "increases"
            | "price"
            | "prices"
            | "raise"
            | "raised"
            | "raises"
            | "tariff"
            | "tariffs"
            | "threat"
            | "threaten"
            | "threatened"
            | "threatens"
    )
}

fn is_cross_language_identity_token(token: &str) -> bool {
    token.is_ascii()
        && (token.len() >= 3
            || (token.len() >= 2 && token.chars().any(|character| character.is_ascii_digit())))
        && !token.starts_with("__")
        && !matches!(
            token,
            "ai" | "api"
                | "app"
                | "apps"
                | "new"
                | "model"
                | "models"
                | "team"
                | "premium"
                | "talk"
                | "talks"
                | "lease"
                | "computing"
                | "power"
                | "deal"
                | "data"
                | "centre"
                | "center"
                | "potential"
                | "billion"
                | "price"
                | "prices"
                | "raise"
                | "raises"
                | "increase"
                | "increases"
                | "cost"
                | "costs"
                | "subscription"
                | "subscriptions"
                | "tariff"
                | "tariffs"
                | "threat"
                | "threaten"
                | "threatens"
                | "threatened"
                | "launch"
                | "launches"
                | "release"
                | "released"
                | "access"
                | "support"
                | "website"
                | "websites"
                | "social"
                | "media"
                | "post"
                | "posts"
                | "company"
                | "startup"
                | "start"
                | "secure"
                | "lets"
                | "seeing"
                | "password"
                | "passwords"
                | "without"
                | "your"
        )
}

fn story_entity_aliases(tokens: &BTreeSet<String>) -> Vec<String> {
    let mut aliases = Vec::new();
    if tokens.contains("__entity_apple_intelligence") {
        aliases.extend(["apple intelligence", "苹果智能"]);
    }
    if tokens.contains("__entity_apple_music") {
        aliases.push("apple music");
    }
    if tokens.contains("__entity_iphone") {
        aliases.push("iphone");
    }
    if tokens.contains("__entity_icloud") {
        aliases.push("icloud");
    }
    if tokens.contains("__entity_apple_services") {
        aliases.extend(["apple music", "apple one", "icloud"]);
    }
    if tokens.contains("__entity_moonshot_kimi") {
        aliases.extend(["moonshot", "kimi", "月之暗面"]);
    }
    if tokens.contains("__entity_trump") {
        aliases.extend(["trump", "特朗普", "川普"]);
    }
    if tokens.contains("__entity_truth_social") {
        aliases.extend(["truth social", "trump media", "真相社交"]);
    }
    if tokens.contains("__entity_canada_wildfire_smoke") {
        aliases.extend([
            "canada wildfire",
            "canadian wildfire",
            "canada smoke",
            "加拿大野火",
            "加拿大山火",
        ]);
    }
    aliases.into_iter().map(str::to_string).collect()
}

fn is_event_action_token(token: &str) -> bool {
    matches!(
        token,
        "备案"
            | "发布"
            | "宣布"
            | "关闭"
            | "批准"
            | "上线"
            | "下线"
            | "暂停"
            | "收购"
            | "起诉"
            | "去世"
            | "死亡"
            | "获救"
            | "撤回"
            | "辞职"
            | "任命"
            | "开放"
            | "禁用"
            | "解雇"
            | "签署"
            | "通过"
            | "appeal"
            | "approve"
            | "attack"
            | "ban"
            | "charge"
            | "close"
            | "conviction"
            | "deal"
            | "death"
            | "deny"
            | "die"
            | "earthquake"
            | "elect"
            | "fire"
            | "flood"
            | "hijack"
            | "hit"
            | "kill"
            | "launch"
            | "leader"
            | "resign"
            | "shoot"
            | "strike"
            | "talk"
            | "threaten"
            | "wound"
    )
}

fn online_story_match_score(
    left: &str,
    right: &str,
    same_key: bool,
    same_source: bool,
    elapsed: Duration,
) -> f32 {
    let left_tokens = story_tokens(left);
    let right_tokens = story_tokens(right);
    story_match_score(
        &left_tokens,
        &character_bigrams(left),
        &right_tokens,
        &character_bigrams(right),
        StoryMatchContext {
            same_key,
            same_url: false,
            same_source,
            elapsed,
        },
    )
}

pub(crate) fn titles_describe_same_event(left: &str, right: &str) -> bool {
    let left_tokens = story_tokens(left);
    let right_tokens = story_tokens(right);
    story_match_score(
        &left_tokens,
        &character_bigrams(left),
        &right_tokens,
        &character_bigrams(right),
        StoryMatchContext {
            same_key: story_key(&left_tokens) == story_key(&right_tokens),
            same_url: false,
            same_source: false,
            elapsed: Duration::zero(),
        },
    ) >= 1.0
}

fn character_bigrams(value: &str) -> BTreeSet<String> {
    let characters = value
        .to_lowercase()
        .chars()
        .filter(|character| character.is_alphanumeric())
        .collect::<Vec<_>>();
    characters
        .windows(2)
        .map(|pair| pair.iter().collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles_match(left: &str, right: &str, same_source: bool, elapsed: Duration) -> bool {
        let left_tokens = story_tokens(left);
        let right_tokens = story_tokens(right);
        story_match_score(
            &left_tokens,
            &character_bigrams(left),
            &right_tokens,
            &character_bigrams(right),
            StoryMatchContext {
                same_key: story_key(&left_tokens) == story_key(&right_tokens),
                same_url: false,
                same_source,
                elapsed,
            },
        ) >= 1.0
    }

    #[test]
    fn similar_titles_have_high_overlap() {
        assert!(titles_match(
            "OpenAI 发布新的 Agent 平台",
            "OpenAI 发布全新 Agent 平台",
            false,
            Duration::hours(1),
        ));
    }

    #[test]
    fn unrelated_titles_do_not_cluster() {
        assert!(!titles_match(
            "Rust 1.90 发布",
            "全球市场今日走势",
            false,
            Duration::hours(1),
        ));
    }

    #[test]
    fn cross_source_paraphrases_cluster() {
        assert!(titles_match(
            "Factory fire kills at least 28 in China's 'shoe capital'",
            "At least 28 people killed as fire engulfs shoe factory in China",
            false,
            Duration::hours(4),
        ));
        assert!(titles_match(
            "Ann Widdecombe, former Tory minister and Reform UK member, dies aged 78",
            "Former Tory minister Ann Widdecombe dies at 78",
            false,
            Duration::hours(8),
        ));
        assert!(titles_match(
            "Jeffrey Donaldson to appeal against conviction for child sexual offences",
            "Former DUP leader Jeffrey Donaldson to appeal child sex abuse convictions",
            false,
            Duration::hours(8),
        ));
        assert!(titles_match(
            "8 killed and more than 60 wounded in Ukrainian drone attack on Russian regions",
            "Ukrainian drone attacks on Russia kill at least 8, wound dozens",
            false,
            Duration::hours(8),
        ));
        assert!(titles_match(
            "Meta in talks for up to $10bn data-centre deal with Anthropic",
            "Meta Is in Talks to Lease Computing Power to Anthropic in a Deal Potentially Worth $10 Billion",
            false,
            Duration::hours(8),
        ));
        assert!(titles_match(
            "Magnitude 7.3 earthquake strikes southern Mexico",
            "Mexico hit by magnitude-7.3 earthquake",
            false,
            Duration::hours(8),
        ));
        assert!(titles_match(
            "Trump threatens Canada with tariffs over invasion of wildfire smoke",
            "Trump threatens new Canada tariffs over fires sending filthy air into US cities",
            false,
            Duration::hours(8),
        ));
        assert!(titles_match(
            "Apple regains top spot as world's most valuable company",
            "Apple briefly leapfrogs Nvidia as world's most valuable company",
            false,
            Duration::hours(8),
        ));
        assert!(titles_match(
            "West Indies legend Sobers dies aged 89",
            "All-time West Indies cricket great Garfield Sobers dies aged 89",
            false,
            Duration::hours(8),
        ));
        assert!(titles_match(
            "Brenda Fricker, Oscar winner for My Left Foot, dies aged 81",
            "My Left Foot and Home Alone 2 actress Brenda Fricker dies at 81",
            false,
            Duration::hours(8),
        ));
    }

    #[test]
    fn shared_generic_event_words_do_not_merge_unrelated_reports() {
        assert!(!titles_match(
            "Canada wildfires affect air quality before World Cup final",
            "Argentina's New York fans share hopes before 2026 World Cup final",
            false,
            Duration::hours(2),
        ));
        assert!(!titles_match(
            "Israeli ministers announce plans for new settlements in Gaza",
            "Burnham to announce plans for North Sea drilling",
            false,
            Duration::hours(2),
        ));
    }

    #[test]
    fn mixed_script_titles_keep_ascii_entity_tokens() {
        let tokens = story_tokens("Linus Torvalds重申：Linux并非反AI项目");
        assert!(tokens.contains("linus"));
        assert!(tokens.contains("torvalds"));
        assert!(tokens.contains("linux"));
        assert!(tokens.contains("ai"));
    }

    #[test]
    fn trump_entity_lookup_covers_common_chinese_names() {
        let tokens = story_tokens("Trump threatens Canada with tariffs");
        let aliases = story_entity_aliases(&tokens);
        assert!(aliases.iter().any(|alias| alias == "特朗普"));
        assert!(aliases.iter().any(|alias| alias == "川普"));
    }

    #[test]
    fn cross_language_named_event_titles_cluster() {
        assert!(titles_match(
            "Linus Torvalds Reaffirms That Linux Is Not Anti-AI & Not A Social Warrior Project",
            "Linus Torvalds重申：Linux并非反AI项目 也不做社会斗士",
            false,
            Duration::hours(8),
        ));
        assert!(!titles_match(
            "Linus Torvalds Reaffirms That Linux Is Not Anti-AI",
            "Linus Torvalds再次批评Nvidia的Linux驱动",
            false,
            Duration::hours(8),
        ));
        assert!(titles_match(
            "Meta in Talks to Lease Computing Power to Anthropic in Potential $10 Billion Deal",
            "Meta据称正洽谈向Anthropic出租算力资源，这笔合作交易两年规模最高或达100亿美元",
            false,
            Duration::hours(2),
        ));
        assert!(titles_match(
            "Meta in Talks to Lease Computing Power to Anthropic in Potential $10 Billion Deal",
            "Anthropic正与Meta就采购算力展开初步谈判，以扩充训练和运行前沿AI模型所需的基础设施。数周前，Anthropic刚宣布与马斯克旗下SpaceX达成类似协议，将使用...",
            false,
            Duration::hours(3),
        ));
        assert!(titles_match(
            "两年最高100亿美元 传Anthropic洽谈从Meta租用算力",
            "Anthropic正与Meta就采购算力展开初步谈判，以扩充训练和运行前沿AI模型所需的基础设施",
            false,
            Duration::hours(3),
        ));
        assert!(titles_match(
            "1Password adds Claude integration for secure AI logins",
            "1Password推出Claude集成：AI可代登录网站但不接触密码",
            false,
            Duration::hours(4),
        ));
        assert!(titles_match(
            "1Password for Claude Lets AI Log In Without Seeing Your Passwords",
            "1Password 推出 Claude 集成：AI 可代登录网站但全程不接触密码",
            false,
            Duration::hours(4),
        ));
        assert!(titles_match(
            "Apple Music raises subscription prices in several countries",
            "苹果Apple Music在中美日等多地区涨价",
            false,
            Duration::hours(4),
        ));
        assert!(titles_match(
            "Apple raises prices for Apple Music and Apple One subscriptions",
            "Apple Music美国区订阅价格上调 家庭方案涨幅明显",
            false,
            Duration::hours(4),
        ));
        assert!(titles_match(
            "Apple increases iPhone prices in Japan by up to 11%",
            "Apple Raises iPhone Prices in Japan",
            false,
            Duration::hours(16),
        ));
        assert!(titles_match(
            "Apple increases iPhone prices in Japan by up to 11%",
            "日元贬值压力增大：苹果公司上调日本市场iPhone售价",
            false,
            Duration::hours(16),
        ));
        assert!(titles_match(
            "Apple increases iPhone prices in Japan by up to 11%",
            "日本iPhone17系列调整价格",
            false,
            Duration::hours(16),
        ));
        assert!(!titles_match(
            "Apple increases iPhone prices in Japan by up to 11%",
            "Starlink 2X Price Increase",
            false,
            Duration::hours(4),
        ));
        assert!(!titles_match(
            "Apple raises prices for Apple Music and Apple One subscriptions",
            "Apple Raises iPhone Prices in Japan",
            false,
            Duration::hours(16),
        ));
        assert!(!titles_match(
            "Apple raises iCloud+ subscription prices in several countries",
            "Apple Raises iPhone Prices in Japan",
            false,
            Duration::hours(8),
        ));
        assert!(titles_match(
            "Apple raises iCloud+ subscription prices in several countries",
            "Apple Music Now Costs $11.99 as Apple Increases Subscription Prices",
            false,
            Duration::hours(8),
        ));
        assert!(!titles_match(
            "Apple TV for Android adds widgets, Apple Music gets WhatsApp sharing",
            "Apple Music美国区订阅价格上调 家庭方案涨幅明显",
            false,
            Duration::hours(26),
        ));
        assert!(!titles_match(
            "iPhone在华市场份额增长 涨价冲击Android竞争对手",
            "Apple increases iPhone prices in Japan by up to 11%",
            false,
            Duration::days(3),
        ));
        assert!(titles_match(
            "Moonshot AI releases open source Kimi K3 model",
            "月之暗面发布开源Kimi K3模型",
            false,
            Duration::hours(2),
        ));
        assert!(titles_match(
            "Chinese AI start-up Moonshot launches model challenging Anthropic’s lead",
            "Kimi K3 发布：开源 2.8T 模型，前端编程在 Arena 中超越 Fable 5 跃居第一",
            false,
            Duration::hours(2),
        ));
        assert!(!titles_match(
            "Kimi K3 released as an open source 2.8T model",
            "Kimi K3首登DeepSWE第三",
            false,
            Duration::hours(2),
        ));
        assert!(titles_match(
            "DeepSWE更新Kimi K3成绩，成为第一个中国模型获得前三名",
            "Kimi K3首登DeepSWE第三",
            false,
            Duration::hours(2),
        ));
        assert!(!titles_match(
            "Kimi K3 发布：2.8T 参数开源模型",
            "OpenCode已上线Kimi K3",
            false,
            Duration::hours(2),
        ));
        assert!(!titles_match(
            "Meta and Anthropic in talks for a computing deal",
            "SpaceX洽谈为五角大楼AI计划提供算力支持",
            false,
            Duration::hours(2),
        ));
        assert!(titles_match(
            "Trump Media to sell high-speed access to president’s social media posts",
            "Truth Social 将向华尔街出售特朗普帖子的快速访问权限",
            false,
            Duration::hours(8),
        ));
        assert!(titles_match(
            "Trump Media to sell instant access to market-moving social posts",
            "川普媒体将向华尔街机构出售 Truth Social 帖子的高速优先访问权限",
            false,
            Duration::hours(4),
        ));
        assert!(titles_match(
            "Trump threatens Canada with steeper tariff costs over wildfire smoke",
            "川普威胁对加拿大追加关税，指责加方未妥善治理大规模山火",
            false,
            Duration::hours(2),
        ));
        assert!(titles_match(
            "Trump threatens Canada with steeper tariff costs over wildfire smoke",
            "加拿大野火烟雾蔓延至美国 特朗普痛批并称要加税",
            false,
            Duration::hours(12),
        ));
        assert!(!titles_match(
            "Trump threatens Canada with steeper tariff costs over wildfire smoke",
            "US refunds $81bn in Trump tariffs after supreme court ruled them illegal",
            false,
            Duration::hours(4),
        ));
        assert!(!titles_match(
            "Trump threatens new Canada tariffs over fires sending filthy air into US cities",
            "Australia to toughen modern slavery penalty after US threatens trade tariffs",
            false,
            Duration::hours(4),
        ));
    }

    #[test]
    fn terse_chinese_event_title_clusters_with_detailed_report() {
        assert!(titles_match(
            "[快讯]苹果智能已备案",
            "[Apple] 苹果智能国内备案，阿里千问能力集成其中",
            false,
            Duration::minutes(30),
        ));
        assert!(!titles_match(
            "[快讯]苹果智能已备案",
            "[快讯]某国产大模型已备案",
            false,
            Duration::minutes(30),
        ));
        assert!(!titles_match(
            "苹果智能国内备案，阿里千问能力集成其中",
            "苹果智能推迟发布，团队继续调整模型",
            false,
            Duration::hours(8),
        ));
    }

    #[test]
    fn apple_intelligence_regulatory_reports_cluster_across_languages() {
        assert!(titles_match(
            "Apple Intelligence Finally Cleared to Launch in China",
            "苹果Apple Intelligence在华已通过监管备案 发布在即",
            false,
            Duration::hours(2),
        ));
        assert!(titles_match(
            "Apple Intelligence Finally Cleared to Launch in China",
            "苹果智能已获准在中国上线，并将接入阿里巴巴的 Qwen 模型",
            false,
            Duration::hours(2),
        ));
        assert!(!titles_match(
            "Apple Intelligence Finally Cleared to Launch in China",
            "Apple Intelligence launch delayed while Apple adjusts the model",
            false,
            Duration::hours(2),
        ));
    }

    #[test]
    fn same_source_marketplace_posts_stay_separate() {
        assert!(!titles_match(
            "[+15R]出VMISS US.LA.9929.Basic 9折",
            "【收】VMISS US.LA.9929.Basic 9 折",
            true,
            Duration::hours(2),
        ));
    }

    #[test]
    fn recurring_same_source_headlines_only_dedupe_within_twelve_hours() {
        assert!(titles_match(
            "Morning news brief",
            "Morning news brief",
            true,
            Duration::minutes(5),
        ));
        assert!(!titles_match(
            "Morning news brief",
            "Morning news brief",
            true,
            Duration::days(1),
        ));
    }

    #[test]
    fn canonical_url_keeps_title_updates_in_one_story() {
        let left_tokens = story_tokens("Factory fire kills at least 28 in China");
        let right_tokens = story_tokens("Death toll rises after China factory blaze");
        assert_eq!(
            story_match_score(
                &left_tokens,
                &character_bigrams("Factory fire kills at least 28 in China"),
                &right_tokens,
                &character_bigrams("Death toll rises after China factory blaze"),
                StoryMatchContext {
                    same_key: false,
                    same_url: true,
                    same_source: true,
                    elapsed: Duration::days(1),
                },
            ),
            1.0
        );
    }

    #[test]
    fn different_country_policy_events_stay_separate() {
        assert!(!titles_match(
            "Britain will ban social media for children under 16",
            "Canada introduces bill to ban social media for children under 16",
            false,
            Duration::hours(2),
        ));
    }

    #[test]
    fn duplicate_feeds_from_one_publisher_share_a_source_key() {
        assert_eq!(
            publisher_family(None, "https://www.bbc.co.uk/news/rss.xml", None, None),
            publisher_family(
                Some("https://bbc.co.uk/news/world"),
                "https://example.com/rss",
                None,
                None,
            )
        );
    }

    #[test]
    fn separate_social_routes_are_independent_sources() {
        assert_ne!(
            publisher_family(
                Some("https://t.me/tnews365"),
                "https://example.com/hub/rsshub/telegram/channel/tnews365",
                Some("rsshub"),
                Some("telegram/channel/tnews365"),
            ),
            publisher_family(
                Some("https://t.me/DNSPODT"),
                "https://example.com/hub/rsshub/telegram/channel/DNSPODT",
                Some("rsshub"),
                Some("telegram/channel/DNSPODT"),
            )
        );
    }

    #[test]
    fn detects_cjk_language() {
        assert_eq!(
            detect_language("中文标题", "这是一篇中文文章"),
            Some("zh".to_string())
        );
        assert_eq!(
            detect_language("Rust release", "New compiler features"),
            Some("en".to_string())
        );
    }

    #[test]
    fn penalizes_markup_script_and_json_noise() {
        let clean = "A carefully reported article with original context and analysis. ".repeat(30);
        let markup = format!("{} <div class=\"error\">blocked</div>", clean);
        let script = format!("{} GoogleAnalyticsObject function(i,s,o,g,r,a,m)", clean);
        let json = format!(
            "{} {{\"one\":1,\"two\":2,\"three\":3,\"four\":4,\"five\":5,\"six\":6,\"seven\":7,\"eight\":8}}",
            clean
        );
        let published_at = Some(Utc::now());
        let clean_score = article_quality_score(&clean, Some("Reporter"), None, published_at);
        assert!(clean_score >= 80);
        assert!(article_quality_score(&markup, Some("Reporter"), None, published_at) <= 55);
        assert!(article_quality_score(&script, Some("Reporter"), None, published_at) <= 45);
        assert!(article_quality_score(&json, Some("Reporter"), None, published_at) <= 50);
    }
}
