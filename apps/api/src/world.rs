use std::collections::HashSet;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{
    auth,
    error::{AppError, AppResult},
    extractor::clean_article_text,
    state::AppState,
};

const MAX_CANDIDATES: i64 = 2_000;
const MIN_BRIEF_SCORE: i32 = 60;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v2/brief/now", get(now_brief))
        .route("/api/v2/wire/now", get(wire_now))
        .route("/api/v2/events/{id}", get(event_detail))
        .route("/api/v2/events/{id}/digest", post(queue_event_digest))
        .route("/api/v2/watches", get(list_watches).post(create_watch))
        .route(
            "/api/v2/watches/{id}",
            axum::routing::patch(update_watch).delete(delete_watch),
        )
        .route("/api/v2/feedback", post(save_feedback))
}

#[derive(Debug, Deserialize)]
struct BriefQuery {
    minutes: Option<i32>,
    hours: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct WireQuery {
    hours: Option<i64>,
    limit: Option<i64>,
}

#[derive(Debug, Serialize)]
struct WireResponse {
    generated_at: DateTime<Utc>,
    window_hours: i64,
    stats: WireStats,
    items: Vec<WireItem>,
}

#[derive(Debug, Serialize)]
struct WireStats {
    published: usize,
    pending: i64,
    running: i64,
    failed: i64,
    skipped: i64,
}

#[derive(Debug, Serialize)]
struct WireItem {
    event_id: Uuid,
    topic: String,
    message: String,
    verification_status: String,
    verification_label: String,
    verification_score: i32,
    verification_reason: String,
    revision_type: String,
    source_count: i32,
    cited_source_count: i32,
    source_titles: Vec<String>,
    citations: serde_json::Value,
    disagreements: Vec<String>,
    published_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct QueueDigestResponse {
    event_id: Uuid,
    queued: bool,
}

#[derive(Debug, Serialize)]
struct BriefResponse {
    generated_at: DateTime<Utc>,
    window_hours: i64,
    budget_minutes: i32,
    stats: BriefStats,
    items: Vec<EventBriefItem>,
}

#[derive(Debug, Serialize)]
struct BriefStats {
    candidate_events: usize,
    eligible_events: usize,
    selected_events: usize,
    independent_sources: usize,
    watch_matches: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct EventBriefItem {
    event_id: Uuid,
    revision_type: String,
    title: String,
    what_happened: String,
    why_it_matters: String,
    why_now: String,
    confidence_label: String,
    confidence_score: i32,
    source_quality_score: i32,
    impact_score: i32,
    novelty_score: i32,
    urgency_score: i32,
    relevance_score: i32,
    total_score: i32,
    source_count: i32,
    independent_source_count: i32,
    source_titles: Vec<String>,
    watch_ids: Vec<Uuid>,
    watch_names: Vec<String>,
    representative_article_id: Uuid,
    published_at: DateTime<Utc>,
    update_count: i32,
    has_grounded_summary: bool,
    feedback: Option<String>,
    #[serde(skip_serializing)]
    community_only: bool,
    #[serde(skip_serializing)]
    content_hash: String,
    #[serde(skip_serializing)]
    url_hash: String,
}

#[derive(Debug, Serialize)]
struct EventDetail {
    event: EventBriefItem,
    timeline: Vec<EventRevision>,
    evidence: Vec<EventEvidence>,
}

#[derive(Debug, Serialize)]
struct EventRevision {
    id: Uuid,
    revision_type: String,
    title: String,
    source_count: i32,
    occurred_at: DateTime<Utc>,
    article_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
struct EventEvidence {
    article_id: Uuid,
    source_title: String,
    title: String,
    url: String,
    excerpt: String,
    quality_score: i32,
    published_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
struct Watch {
    id: Uuid,
    name: String,
    query: String,
    min_impact: i32,
    notification_mode: String,
    enabled: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct WatchRequest {
    name: String,
    query: String,
    #[serde(default = "default_min_impact")]
    min_impact: i32,
    #[serde(default = "default_notification_mode")]
    notification_mode: String,
    #[serde(default = "default_true")]
    enabled: bool,
}

#[derive(Debug, Deserialize)]
struct FeedbackRequest {
    event_id: Uuid,
    kind: String,
}

#[derive(Debug, Serialize)]
struct FeedbackResponse {
    event_id: Uuid,
    kind: String,
}

#[derive(Debug)]
struct Candidate {
    event_id: Uuid,
    title: String,
    representative_article_id: Uuid,
    source_titles: Vec<String>,
    source_count: i32,
    independent_source_count: i32,
    article_count: i32,
    update_count: i32,
    average_source_quality: i32,
    published_at: DateTime<Utc>,
    summary: Option<String>,
    summary_confidence: Option<String>,
    grounding_score: Option<i32>,
    excerpt: String,
    revision_type: String,
    feedback: Option<String>,
    content_hash: String,
    url_hash: String,
}

#[derive(Debug)]
struct PreferenceSignal {
    event_id: Uuid,
    kind: String,
    title: String,
}

async fn now_brief(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<BriefQuery>,
) -> AppResult<Json<BriefResponse>> {
    auth::require_session(&state.pool, &headers).await?;
    let minutes = normalize_budget(query.minutes.unwrap_or(10));
    let hours = query.hours.unwrap_or(24).clamp(1, 24 * 30);
    let limit = budget_limit(minutes);
    let watches = enabled_watches(&state).await?;
    let preferences = load_preference_signals(&state).await?;
    let candidates = load_candidates(&state, hours, None).await?;
    let candidate_count = candidates.len();
    let ranked = candidates
        .into_iter()
        .map(|candidate| score_candidate(candidate, &watches, &preferences))
        .collect::<Vec<_>>();
    let eligible_events = ranked
        .iter()
        .filter(|item| passes_evidence_gate(item))
        .count();
    let items = select_diverse(ranked, limit);
    let independent_sources = items
        .iter()
        .flat_map(|item| item.source_titles.iter().cloned())
        .collect::<HashSet<_>>()
        .len();
    let watch_matches = items
        .iter()
        .filter(|item| !item.watch_ids.is_empty())
        .count();

    Ok(Json(BriefResponse {
        generated_at: Utc::now(),
        window_hours: hours,
        budget_minutes: minutes,
        stats: BriefStats {
            candidate_events: candidate_count,
            eligible_events,
            selected_events: items.len(),
            independent_sources,
            watch_matches,
        },
        items,
    }))
}

async fn wire_now(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<WireQuery>,
) -> AppResult<Json<WireResponse>> {
    auth::require_session(&state.pool, &headers).await?;
    let hours = query.hours.unwrap_or(24).clamp(1, 24 * 30);
    let limit = query.limit.unwrap_or(30).clamp(1, 100);
    let rows = sqlx::query(
        "SELECT story_id, topic, message, verification_status, verification_label,
                verification_score, verification_reason, revision_type,
                source_count, cited_source_count, source_titles, citations,
                disagreements, published_at, updated_at
         FROM event_digests
         WHERE status = 'done'
           AND published_at >= now() - ($1 || ' hours')::interval
           AND EXISTS (
             SELECT 1
             FROM articles quality_article
             JOIN feeds quality_feed ON quality_feed.id = quality_article.feed_id
             WHERE quality_article.story_id = event_digests.story_id
               AND quality_feed.quality_score >= 70
           )
         ORDER BY verification_score DESC,
                  cited_source_count DESC,
                  published_at DESC,
                  updated_at DESC
         LIMIT $2",
    )
    .bind(hours)
    .bind(limit)
    .fetch_all(&state.pool)
    .await?;
    let items = rows
        .into_iter()
        .map(|row| WireItem {
            event_id: row.get("story_id"),
            topic: row.get("topic"),
            message: row.get("message"),
            verification_status: row.get("verification_status"),
            verification_label: row.get("verification_label"),
            verification_score: row.get("verification_score"),
            verification_reason: row.get("verification_reason"),
            revision_type: row.get("revision_type"),
            source_count: row.get("source_count"),
            cited_source_count: row.get("cited_source_count"),
            source_titles: sorted_unique(row.get("source_titles")),
            citations: row.get("citations"),
            disagreements: serde_json::from_value(row.get("disagreements")).unwrap_or_default(),
            published_at: row.get("published_at"),
            updated_at: row.get("updated_at"),
        })
        .collect::<Vec<_>>();
    let status = sqlx::query(
        "SELECT COUNT(*) FILTER (WHERE status = 'pending')::bigint AS pending,
                COUNT(*) FILTER (WHERE status = 'running')::bigint AS running,
                COUNT(*) FILTER (WHERE status = 'failed')::bigint AS failed,
                COUNT(*) FILTER (WHERE status = 'skipped')::bigint AS skipped
         FROM event_digests",
    )
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(WireResponse {
        generated_at: Utc::now(),
        window_hours: hours,
        stats: WireStats {
            published: items.len(),
            pending: status.get("pending"),
            running: status.get("running"),
            failed: status.get("failed"),
            skipped: status.get("skipped"),
        },
        items,
    }))
}

async fn queue_event_digest(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> AppResult<Json<QueueDigestResponse>> {
    auth::require_session(&state.pool, &headers).await?;
    let queued = crate::event_agent::queue_story(&state, id).await?;
    Ok(Json(QueueDigestResponse {
        event_id: id,
        queued,
    }))
}

async fn event_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> AppResult<Json<EventDetail>> {
    auth::require_session(&state.pool, &headers).await?;
    let watches = enabled_watches(&state).await?;
    let preferences = load_preference_signals(&state).await?;
    let candidate = load_candidates(&state, 24 * 365, Some(id))
        .await?
        .into_iter()
        .next()
        .ok_or(AppError::NotFound)?;
    let event = score_candidate(candidate, &watches, &preferences);
    let revision_rows = sqlx::query(
        "SELECT id, revision_type, title, source_count, occurred_at, article_id
         FROM event_revisions
         WHERE story_id = $1
         ORDER BY occurred_at DESC, created_at DESC
         LIMIT 100",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;
    let evidence_rows = sqlx::query(
        "SELECT a.id, f.title AS source_title, a.title, a.url,
                COALESCE(NULLIF(a.extracted_content, ''), a.content) AS body,
                a.quality_score, COALESCE(a.published_at, a.fetched_at) AS published_at
         FROM articles a
         JOIN feeds f ON f.id = a.feed_id
         LEFT JOIN source_feedback sf ON sf.feed_id = f.id
         WHERE a.story_id = $1 AND COALESCE(sf.muted, false) = false
         ORDER BY a.quality_score DESC, COALESCE(a.published_at, a.fetched_at) DESC
         LIMIT 30",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(EventDetail {
        event,
        timeline: revision_rows
            .into_iter()
            .map(|row| EventRevision {
                id: row.get("id"),
                revision_type: row.get("revision_type"),
                title: row.get("title"),
                source_count: row.get("source_count"),
                occurred_at: row.get("occurred_at"),
                article_id: row.get("article_id"),
            })
            .collect(),
        evidence: evidence_rows
            .into_iter()
            .map(|row| {
                let body = row.get::<String, _>("body");
                let title = row.get::<String, _>("title");
                let cleaned = clean_article_text(&body, &title);
                EventEvidence {
                    article_id: row.get("id"),
                    source_title: row.get("source_title"),
                    title,
                    url: row.get("url"),
                    excerpt: plain_excerpt(&cleaned, 220),
                    quality_score: row.get("quality_score"),
                    published_at: row.get("published_at"),
                }
            })
            .collect(),
    }))
}

async fn list_watches(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<Vec<Watch>>> {
    auth::require_session(&state.pool, &headers).await?;
    Ok(Json(all_watches(&state).await?))
}

async fn create_watch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<WatchRequest>,
) -> AppResult<(StatusCode, Json<Watch>)> {
    auth::require_session(&state.pool, &headers).await?;
    validate_watch(&body)?;
    let row = sqlx::query(
        "INSERT INTO watches (name, query, min_impact, notification_mode, enabled)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING id, name, query, min_impact, notification_mode, enabled, created_at, updated_at",
    )
    .bind(body.name.trim())
    .bind(body.query.trim())
    .bind(body.min_impact)
    .bind(&body.notification_mode)
    .bind(body.enabled)
    .fetch_one(&state.pool)
    .await?;
    Ok((StatusCode::CREATED, Json(watch_from_row(row))))
}

async fn update_watch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<WatchRequest>,
) -> AppResult<Json<Watch>> {
    auth::require_session(&state.pool, &headers).await?;
    validate_watch(&body)?;
    let row = sqlx::query(
        "UPDATE watches
         SET name = $2, query = $3, min_impact = $4, notification_mode = $5,
             enabled = $6, updated_at = now()
         WHERE id = $1
         RETURNING id, name, query, min_impact, notification_mode, enabled, created_at, updated_at",
    )
    .bind(id)
    .bind(body.name.trim())
    .bind(body.query.trim())
    .bind(body.min_impact)
    .bind(&body.notification_mode)
    .bind(body.enabled)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(Json(watch_from_row(row)))
}

async fn delete_watch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    auth::require_session(&state.pool, &headers).await?;
    let deleted = sqlx::query("DELETE FROM watches WHERE id = $1")
        .bind(id)
        .execute(&state.pool)
        .await?;
    if deleted.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn save_feedback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<FeedbackRequest>,
) -> AppResult<Json<FeedbackResponse>> {
    auth::require_session(&state.pool, &headers).await?;
    if !matches!(
        body.kind.as_str(),
        "more" | "less" | "known" | "irrelevant" | "bad_cluster"
    ) {
        return Err(AppError::BadRequest("unknown feedback kind".to_string()));
    }
    let exists =
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM story_clusters WHERE id = $1)")
            .bind(body.event_id)
            .fetch_one(&state.pool)
            .await?;
    if !exists {
        return Err(AppError::NotFound);
    }
    sqlx::query(
        "INSERT INTO event_feedback (story_id, kind)
         VALUES ($1, $2)
         ON CONFLICT (story_id) DO UPDATE
         SET kind = EXCLUDED.kind, updated_at = now()",
    )
    .bind(body.event_id)
    .bind(&body.kind)
    .execute(&state.pool)
    .await?;
    Ok(Json(FeedbackResponse {
        event_id: body.event_id,
        kind: body.kind,
    }))
}

async fn load_candidates(
    state: &AppState,
    hours: i64,
    event_id: Option<Uuid>,
) -> AppResult<Vec<Candidate>> {
    let rows = sqlx::query(
        "WITH recent_stories AS (
           SELECT sc.id, sc.representative_title, sc.source_count
           FROM story_clusters sc
           WHERE ($1::uuid IS NOT NULL AND sc.id = $1)
              OR ($1::uuid IS NULL AND COALESCE(sc.last_published_at, sc.updated_at) >= now() - ($2 || ' hours')::interval)
           ORDER BY COALESCE(sc.last_published_at, sc.updated_at) DESC
           LIMIT CASE WHEN $1::uuid IS NULL THEN $3 ELSE 1 END
         ),
         story_signals AS (
           SELECT sc.id, sc.representative_title,
                  GREATEST(sc.source_count, 1) AS source_count,
                  COUNT(a.id)::integer AS article_count,
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
                  END)::integer AS independent_source_count,
                  COUNT(*) FILTER (
                    WHERE COALESCE(a.published_at, a.fetched_at) >= now() - interval '6 hours'
                  )::integer AS update_count,
                  ROUND(AVG(LEAST(
                    100,
                    GREATEST(0, f.quality_score + COALESCE(sf.weight, 0))
                  )))::integer AS average_source_quality,
                  ARRAY_AGG(DISTINCT f.title) AS source_titles,
                  MAX(COALESCE(a.published_at, a.fetched_at)) AS published_at
           FROM recent_stories sc
           JOIN articles a ON a.story_id = sc.id
           JOIN feeds f ON f.id = a.feed_id
           LEFT JOIN source_feedback sf ON sf.feed_id = f.id
           WHERE COALESCE(sf.muted, false) = false
             AND ($1::uuid IS NOT NULL OR COALESCE(a.published_at, a.fetched_at) >= now() - ($2 || ' hours')::interval)
           GROUP BY sc.id, sc.representative_title, sc.source_count
         )
         SELECT ss.*,
                rep.id AS representative_article_id,
                rep.title, rep.content_hash, rep.url_hash,
                CASE
                  WHEN $1::uuid IS NOT NULL
                    THEN COALESCE(NULLIF(rep.extracted_content, ''), rep.content)
                  ELSE LEFT(COALESCE(NULLIF(rep.extracted_content, ''), rep.content), 8000)
                END AS body,
                s.summary, s.confidence AS summary_confidence, s.grounding_score,
                COALESCE(rev.revision_type,
                  CASE WHEN ss.article_count > 1 THEN 'UPDATE' ELSE 'ADD' END
                ) AS revision_type,
                ef.kind AS feedback
         FROM story_signals ss
         JOIN LATERAL (
           SELECT a.id, a.title, a.extracted_content, a.content,
                  a.content_hash, a.url_hash
           FROM articles a
           JOIN feeds rep_feed ON rep_feed.id = a.feed_id
           LEFT JOIN source_feedback rep_feedback ON rep_feedback.feed_id = rep_feed.id
           LEFT JOIN event_revisions rep_revision ON rep_revision.article_id = a.id
           WHERE a.story_id = ss.id
             AND COALESCE(rep_feedback.muted, false) = false
           ORDER BY rep_revision.occurred_at DESC NULLS LAST,
                    a.quality_score DESC,
                    COALESCE(a.published_at, a.fetched_at) DESC
           LIMIT 1
         ) rep ON true
         LEFT JOIN summaries s ON s.article_id = rep.id AND s.status = 'done'
         LEFT JOIN LATERAL (
           SELECT revision_type
           FROM event_revisions er
           WHERE er.story_id = ss.id
           ORDER BY er.occurred_at DESC, er.created_at DESC
           LIMIT 1
         ) rev ON true
         LEFT JOIN event_feedback ef ON ef.story_id = ss.id
         ORDER BY ss.published_at DESC, ss.independent_source_count DESC
         LIMIT $3",
    )
    .bind(event_id)
    .bind(hours)
    .bind(if event_id.is_some() { 1 } else { MAX_CANDIDATES })
    .fetch_all(&state.pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let body = row.get::<String, _>("body");
            let title = row.get::<String, _>("title");
            let cleaned = clean_article_text(&body, &title);
            Candidate {
                event_id: row.get("id"),
                title,
                representative_article_id: row.get("representative_article_id"),
                source_titles: row.get("source_titles"),
                source_count: row.get("source_count"),
                independent_source_count: row.get("independent_source_count"),
                article_count: row.get("article_count"),
                update_count: row.get("update_count"),
                average_source_quality: row.get("average_source_quality"),
                published_at: row.get("published_at"),
                summary: row.get("summary"),
                summary_confidence: row.get("summary_confidence"),
                grounding_score: row.get("grounding_score"),
                excerpt: plain_excerpt(&cleaned, 360),
                revision_type: row.get("revision_type"),
                feedback: row.get("feedback"),
                content_hash: row.get("content_hash"),
                url_hash: row.get("url_hash"),
            }
        })
        .collect())
}

async fn load_preference_signals(state: &AppState) -> AppResult<Vec<PreferenceSignal>> {
    let rows = sqlx::query(
        "SELECT ef.story_id, ef.kind, sc.representative_title
         FROM event_feedback ef
         JOIN story_clusters sc ON sc.id = ef.story_id
         WHERE ef.kind IN ('more', 'less', 'irrelevant')
           AND ef.updated_at >= now() - interval '180 days'
         ORDER BY ef.updated_at DESC
         LIMIT 200",
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| PreferenceSignal {
            event_id: row.get("story_id"),
            kind: row.get("kind"),
            title: row.get("representative_title"),
        })
        .collect())
}

fn score_candidate(
    candidate: Candidate,
    watches: &[Watch],
    preferences: &[PreferenceSignal],
) -> EventBriefItem {
    let age_hours = (Utc::now() - candidate.published_at).num_minutes().max(0) as f32 / 60.0;
    let urgency = freshness_score(age_hours, candidate.update_count, &candidate.revision_type);
    let novelty = change_novelty_score(
        &candidate.revision_type,
        candidate.article_count,
        candidate.update_count,
    );
    let impact = impact_signal(&candidate.title, &candidate.excerpt);
    let confidence = confidence_score(
        candidate.independent_source_count,
        candidate.average_source_quality,
        candidate.grounding_score,
    );
    let (watch_ids, watch_names) = matching_watches(&candidate, watches, impact);
    let mut relevance = if watch_ids.is_empty() { 35 } else { 90 };
    relevance += match candidate.feedback.as_deref() {
        Some("more") => 10,
        Some("less") => -25,
        Some("known") => 0,
        Some("irrelevant") => -40,
        Some("bad_cluster") => -50,
        _ => 0,
    };
    relevance += preference_delta(&candidate, preferences);
    relevance = relevance.clamp(0, 100);
    let total = (impact as f32 * 0.28
        + novelty.clamp(0, 100) as f32 * 0.20
        + urgency.clamp(0, 100) as f32 * 0.18
        + confidence as f32 * 0.19
        + relevance as f32 * 0.15)
        .round() as i32;
    let has_grounded_summary = candidate.summary.is_some();
    let what_happened = candidate
        .summary
        .filter(|summary| !summary.trim().is_empty())
        .unwrap_or_else(|| candidate.excerpt.clone());
    let why_it_matters = importance_reason(
        impact,
        candidate.independent_source_count,
        candidate.average_source_quality,
        &watch_names,
    );
    let why_now = change_reason(&candidate.revision_type, candidate.update_count, age_hours);
    let confidence_label = confidence_label(
        confidence,
        candidate.independent_source_count,
        candidate.summary_confidence.as_deref(),
    );
    let community_only = sources_are_community_only(&candidate.source_titles);

    EventBriefItem {
        event_id: candidate.event_id,
        revision_type: candidate.revision_type,
        title: candidate.title,
        what_happened,
        why_it_matters,
        why_now,
        confidence_label,
        confidence_score: confidence,
        source_quality_score: candidate.average_source_quality,
        impact_score: impact,
        novelty_score: novelty.clamp(0, 100),
        urgency_score: urgency.clamp(0, 100),
        relevance_score: relevance,
        total_score: total.clamp(0, 100),
        source_count: candidate.source_count,
        independent_source_count: candidate.independent_source_count,
        source_titles: sorted_unique(candidate.source_titles),
        watch_ids,
        watch_names,
        representative_article_id: candidate.representative_article_id,
        published_at: candidate.published_at,
        update_count: candidate.update_count,
        has_grounded_summary,
        feedback: candidate.feedback,
        community_only,
        content_hash: candidate.content_hash,
        url_hash: candidate.url_hash,
    }
}

fn select_diverse(mut items: Vec<EventBriefItem>, limit: usize) -> Vec<EventBriefItem> {
    items.retain(passes_evidence_gate);
    let mut selected = Vec::<EventBriefItem>::with_capacity(limit);
    while !items.is_empty() && selected.len() < limit {
        items.retain(|item| {
            !selected.iter().any(|selected_item| {
                selected_item.content_hash == item.content_hash
                    || selected_item.url_hash == item.url_hash
                    || titles_same_brief_event(&selected_item.title, &item.title)
            })
        });
        let Some(best_index) = items
            .iter()
            .enumerate()
            .max_by(|(_, left), (_, right)| {
                selection_score(left, &selected, limit)
                    .total_cmp(&selection_score(right, &selected, limit))
                    .then_with(|| left.total_score.cmp(&right.total_score))
                    .then_with(|| left.published_at.cmp(&right.published_at))
            })
            .map(|(index, _)| index)
        else {
            break;
        };
        selected.push(items.swap_remove(best_index));
    }
    selected
}

fn passes_evidence_gate(item: &EventBriefItem) -> bool {
    if matches!(item.feedback.as_deref(), Some("irrelevant" | "bad_cluster")) {
        return false;
    }
    if !item.watch_ids.is_empty()
        && item.total_score >= 55
        && (item.independent_source_count >= 2
            || item.source_quality_score >= 55
            || (!item.community_only && item.confidence_score >= 40))
    {
        return true;
    }
    if item.independent_source_count >= 2 {
        return item.total_score >= 58;
    }
    if matches!(item.revision_type.as_str(), "CORRECT" | "CONTRADICT")
        && item.source_quality_score >= 55
    {
        return item.total_score >= 58;
    }
    if item.source_quality_score >= 70 {
        return item.total_score >= MIN_BRIEF_SCORE;
    }
    if !item.community_only {
        return item.total_score >= MIN_BRIEF_SCORE;
    }
    item.total_score >= 68 && item.impact_score >= 65 && item.confidence_score >= 40
}

fn selection_score(item: &EventBriefItem, selected: &[EventBriefItem], limit: usize) -> f32 {
    let mut score = item.total_score as f32;
    if item.independent_source_count >= 2 {
        score += 4.0;
    }
    if item.has_grounded_summary {
        score += 2.0;
    }
    if !item.watch_ids.is_empty() {
        score += 7.0;
    }
    if matches!(item.revision_type.as_str(), "CORRECT" | "CONTRADICT") {
        score += 6.0;
    }
    if item.feedback.as_deref() == Some("known") {
        score -= 30.0;
    }
    if selected.is_empty() {
        return score;
    }

    let max_similarity = selected
        .iter()
        .map(|selected_item| brief_title_similarity(&selected_item.title, &item.title))
        .fold(0.0_f32, f32::max);
    score -= max_similarity * 22.0;

    let topic = topic_bucket(&format!("{} {}", item.title, item.what_happened));
    let topic_count = selected
        .iter()
        .filter(|selected_item| {
            topic_bucket(&format!(
                "{} {}",
                selected_item.title, selected_item.what_happened
            )) == topic
        })
        .count();
    let topic_cap = if topic == "general" {
        (limit / 3).max(2)
    } else {
        2
    };
    if topic_count >= topic_cap {
        score -= ((topic_count - topic_cap + 1) as f32) * 10.0;
    }

    let normalized_sources = item
        .source_titles
        .iter()
        .map(|source| source.trim().to_lowercase())
        .collect::<HashSet<_>>();
    if !normalized_sources.is_empty() {
        let repeat_counts = normalized_sources
            .iter()
            .map(|source| {
                selected
                    .iter()
                    .filter(|selected_item| {
                        selected_item.source_titles.iter().any(|selected_source| {
                            selected_source.trim().eq_ignore_ascii_case(source)
                        })
                    })
                    .count()
            })
            .collect::<Vec<_>>();
        let repeated = repeat_counts.iter().filter(|count| **count > 0).count();
        let max_repeat = repeat_counts.into_iter().max().unwrap_or_default();
        score -= max_repeat as f32 * 6.0;
        score -= repeated as f32 / normalized_sources.len() as f32 * 8.0;
    }
    score
}

fn titles_same_brief_event(left: &str, right: &str) -> bool {
    let left = brief_title_tokens(left);
    let right = brief_title_tokens(right);
    if left.len() < 3 || right.len() < 3 {
        return false;
    }
    let shared = left.intersection(&right).count();
    let union = left.union(&right).count();
    shared >= 3 && shared as f32 / union.max(1) as f32 >= 0.20
}

fn brief_title_similarity(left: &str, right: &str) -> f32 {
    let left = brief_title_tokens(left);
    let right = brief_title_tokens(right);
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    left.intersection(&right).count() as f32 / left.union(&right).count().max(1) as f32
}

fn preference_delta(candidate: &Candidate, preferences: &[PreferenceSignal]) -> i32 {
    let mut strongest_positive = 0;
    let mut strongest_negative = 0;
    let candidate_topic = topic_bucket(&format!("{} {}", candidate.title, candidate.excerpt));
    for signal in preferences {
        if signal.event_id == candidate.event_id {
            continue;
        }
        let title_similarity = brief_title_similarity(&candidate.title, &signal.title);
        if title_similarity <= 0.0 {
            continue;
        }
        let same_topic = topic_bucket(&signal.title) == candidate_topic;
        let similarity =
            (title_similarity * 0.85 + if same_topic { 0.15 } else { 0.0 }).clamp(0.0, 1.0);
        let adjustment = match signal.kind.as_str() {
            "more" => (similarity * 18.0).round() as i32,
            "less" => -((similarity * 22.0).round() as i32),
            "irrelevant" => -((similarity * 32.0).round() as i32),
            _ => 0,
        };
        strongest_positive = strongest_positive.max(adjustment);
        strongest_negative = strongest_negative.min(adjustment);
    }
    (strongest_positive + strongest_negative).clamp(-35, 20)
}

fn brief_title_tokens(value: &str) -> HashSet<String> {
    const STOP_WORDS: &[&str] = &[
        "after", "amid", "from", "into", "live", "more", "over", "says", "the", "with",
    ];
    let mut tokens = HashSet::new();
    let mut word = String::new();
    let mut cjk_run = Vec::new();
    let flush_word = |word: &mut String, tokens: &mut HashSet<String>| {
        if word.chars().count() >= 3 && !STOP_WORDS.contains(&word.as_str()) {
            tokens.insert(std::mem::take(word));
        } else {
            word.clear();
        }
    };
    let flush_cjk = |run: &mut Vec<char>, tokens: &mut HashSet<String>| {
        for pair in run.windows(2) {
            tokens.insert(pair.iter().collect());
        }
        run.clear();
    };
    for character in value.to_lowercase().chars() {
        if is_cjk(character) {
            flush_word(&mut word, &mut tokens);
            cjk_run.push(character);
        } else if character.is_alphanumeric() {
            flush_cjk(&mut cjk_run, &mut tokens);
            word.push(character);
        } else {
            flush_word(&mut word, &mut tokens);
            flush_cjk(&mut cjk_run, &mut tokens);
        }
    }
    flush_word(&mut word, &mut tokens);
    flush_cjk(&mut cjk_run, &mut tokens);
    tokens
}

fn is_cjk(character: char) -> bool {
    matches!(character as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF)
}

fn sources_are_community_only(source_titles: &[String]) -> bool {
    !source_titles.is_empty()
        && source_titles.iter().all(|source| {
            let normalized = source.to_lowercase();
            ["v2ex", "nodeseek", "linux do", "linux.do"]
                .iter()
                .any(|marker| normalized.contains(marker))
        })
}

fn matching_watches(
    candidate: &Candidate,
    watches: &[Watch],
    impact: i32,
) -> (Vec<Uuid>, Vec<String>) {
    let haystack = format!("{} {}", candidate.title, candidate.excerpt).to_lowercase();
    let mut ids = Vec::new();
    let mut names = Vec::new();
    for watch in watches {
        if impact < watch.min_impact {
            continue;
        }
        let terms = watch_terms(&watch.query);
        if watch_query_matches(&haystack, &terms) {
            ids.push(watch.id);
            names.push(watch.name.clone());
        }
    }
    (ids, names)
}

fn watch_terms(query: &str) -> Vec<String> {
    query
        .split(|character: char| character.is_whitespace() || ",，;；|".contains(character))
        .map(str::trim)
        .filter(|term| term.chars().count() >= 2)
        .map(str::to_lowercase)
        .collect()
}

fn watch_query_matches(haystack: &str, terms: &[String]) -> bool {
    if terms.is_empty() {
        return false;
    }
    let matched = terms
        .iter()
        .filter(|term| {
            if term.is_ascii() {
                contains_signal_marker(haystack, term)
            } else {
                haystack.contains(term.as_str())
            }
        })
        .count();
    let required = if terms.len() == 1 {
        1
    } else {
        terms.len().div_ceil(2).max(2)
    };
    matched >= required
}

fn impact_signal(title: &str, excerpt: &str) -> i32 {
    let haystack = format!("{title} {excerpt}").to_lowercase();
    let high = [
        "war",
        "strike",
        "strikes",
        "attack",
        "military",
        "election",
        "government",
        "regulation",
        "sanction",
        "earthquake",
        "outage",
        "breach",
        "vulnerability",
        "launches",
        "acquires",
        "bankruptcy",
        "战争",
        "空袭",
        "袭击",
        "军事",
        "选举",
        "政府",
        "监管",
        "制裁",
        "地震",
        "宕机",
        "漏洞",
        "发布",
        "收购",
        "破产",
        "危机",
        "冲突",
    ];
    let medium = [
        "ai",
        "chip",
        "security",
        "economy",
        "market",
        "research",
        "climate",
        "人工智能",
        "芯片",
        "安全",
        "经济",
        "市场",
        "研究",
        "气候",
    ];
    let high_matches = high
        .iter()
        .filter(|marker| contains_signal_marker(&haystack, marker))
        .count() as i32;
    let medium_matches = medium
        .iter()
        .filter(|marker| contains_signal_marker(&haystack, marker))
        .count() as i32;
    (44 + high_matches * 14 + medium_matches * 7).clamp(0, 100)
}

fn contains_signal_marker(haystack: &str, marker: &str) -> bool {
    if !marker.is_ascii() {
        return haystack.contains(marker);
    }
    haystack
        .split(|character: char| !character.is_ascii_alphanumeric())
        .any(|token| token == marker)
}

fn confidence_score(independent_sources: i32, source_quality: i32, grounding: Option<i32>) -> i32 {
    let source_signal = match independent_sources {
        0..=1 => 28,
        2 => 55,
        3 => 70,
        4..=5 => 82,
        _ => 90,
    };
    let grounding = grounding.unwrap_or(45).clamp(0, 100);
    (source_signal as f32 * 0.45 + source_quality as f32 * 0.30 + grounding as f32 * 0.25).round()
        as i32
}

fn change_novelty_score(revision_type: &str, article_count: i32, updates: i32) -> i32 {
    let revision_signal = match revision_type {
        "CORRECT" | "CONTRADICT" => 96,
        "UPDATE" => 88,
        "CONFIRM" => 84,
        _ => 82,
    };
    let update_bonus = updates.clamp(0, 3) * 3;
    let repetition_penalty = (article_count - 1).clamp(0, 5) * 2;
    (revision_signal + update_bonus - repetition_penalty).clamp(0, 100)
}

fn freshness_score(age_hours: f32, updates: i32, revision_type: &str) -> i32 {
    let continuous_decay = 100.0 * 2.0_f32.powf(-age_hours.max(0.0) / 12.0);
    let update_bonus = updates.clamp(0, 3) * 4;
    let revision_bonus = if matches!(revision_type, "CORRECT" | "CONTRADICT") {
        6
    } else {
        0
    };
    (continuous_decay.round() as i32 + update_bonus + revision_bonus).clamp(0, 100)
}

fn importance_reason(impact: i32, sources: i32, source_quality: i32, watches: &[String]) -> String {
    if let Some(watch) = watches.first() {
        return format!("命中关注「{watch}」，影响评分 {impact}");
    }
    if sources >= 3 {
        return format!("影响评分 {impact}，已有 {sources} 个订阅源出现相关报道");
    }
    if source_quality >= 70 {
        return format!("来源质量 {source_quality}，影响评分 {impact}，等待更多来源交叉确认");
    }
    if impact >= 75 {
        return format!("潜在影响较大，影响评分 {impact}");
    }
    "通过证据门槛，并在变化、时效、影响与来源多样性重排中靠前".to_string()
}

fn change_reason(revision_type: &str, updates: i32, age_hours: f32) -> String {
    match revision_type {
        "CORRECT" => "出现关键纠正，需要更新原有判断".to_string(),
        "CONTRADICT" => "出现来源矛盾，核心事实仍需核验".to_string(),
        "CONFIRM" => "新增订阅源支持了已有主张".to_string(),
        "UPDATE" if updates > 1 => format!("近 6 小时新增 {updates} 条进展"),
        "UPDATE" => "持续事件出现了新进展".to_string(),
        _ if age_hours <= 2.0 => "首次出现于最近 2 小时".to_string(),
        _ => "这是当前时间窗内首次进入简报的事件".to_string(),
    }
}

fn confidence_label(score: i32, sources: i32, summary_confidence: Option<&str>) -> String {
    let level = if score >= 75 {
        "较高"
    } else if score >= 55 {
        "中等"
    } else {
        "有限"
    };
    match (sources, summary_confidence) {
        (value, Some(model_level)) if value >= 2 => {
            format!("{level} · {value} 源 · 摘要 {model_level}")
        }
        (value, _) if value >= 2 => format!("{level} · {value} 个订阅源"),
        _ => format!("{level} · 单一来源，等待交叉确认"),
    }
}

fn topic_bucket(title: &str) -> &'static str {
    let title = title.to_lowercase();
    if [
        "ai",
        "tech",
        "technology",
        "software",
        "chip",
        "人工智能",
        "科技",
        "芯片",
    ]
    .iter()
    .any(|marker| contains_signal_marker(&title, marker))
    {
        "technology"
    } else if ["market", "economy", "business", "经济", "市场", "公司"]
        .iter()
        .any(|marker| contains_signal_marker(&title, marker))
    {
        "business"
    } else if [
        "security",
        "vulnerability",
        "breach",
        "安全",
        "漏洞",
        "攻击",
    ]
    .iter()
    .any(|marker| contains_signal_marker(&title, marker))
    {
        "security"
    } else if [
        "war",
        "strike",
        "strikes",
        "attack",
        "attacks",
        "military",
        "election",
        "government",
        "战争",
        "空袭",
        "袭击",
        "军事",
        "选举",
        "政府",
        "外交",
    ]
    .iter()
    .any(|marker| contains_signal_marker(&title, marker))
    {
        "world"
    } else {
        "general"
    }
}

pub async fn record_event_revision(
    transaction: &mut Transaction<'_, Postgres>,
    story_id: Uuid,
    article_id: Uuid,
    title: &str,
    occurred_at: DateTime<Utc>,
    is_new_story: bool,
) -> AppResult<()> {
    let source_count = sqlx::query_scalar::<_, i32>(
        "SELECT GREATEST(source_count, 1) FROM story_clusters WHERE id = $1",
    )
    .bind(story_id)
    .fetch_one(&mut **transaction)
    .await?;
    let revision_type = classify_revision(title, is_new_story, source_count);
    sqlx::query(
        "INSERT INTO event_revisions
           (story_id, article_id, revision_type, title, source_count, occurred_at)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (article_id) WHERE article_id IS NOT NULL DO UPDATE SET
           story_id = EXCLUDED.story_id,
           revision_type = CASE
             WHEN event_revisions.story_id IS DISTINCT FROM EXCLUDED.story_id
               OR event_revisions.title IS DISTINCT FROM EXCLUDED.title
             THEN EXCLUDED.revision_type
             ELSE event_revisions.revision_type
           END,
           title = EXCLUDED.title,
           source_count = CASE
             WHEN event_revisions.story_id IS DISTINCT FROM EXCLUDED.story_id
               OR event_revisions.title IS DISTINCT FROM EXCLUDED.title
             THEN EXCLUDED.source_count
             ELSE event_revisions.source_count
           END,
           occurred_at = CASE
             WHEN event_revisions.story_id IS DISTINCT FROM EXCLUDED.story_id
               OR event_revisions.title IS DISTINCT FROM EXCLUDED.title
             THEN clock_timestamp()
             ELSE event_revisions.occurred_at
           END,
           created_at = CASE
             WHEN event_revisions.story_id IS DISTINCT FROM EXCLUDED.story_id
               OR event_revisions.title IS DISTINCT FROM EXCLUDED.title
             THEN clock_timestamp()
             ELSE event_revisions.created_at
           END",
    )
    .bind(story_id)
    .bind(article_id)
    .bind(revision_type)
    .bind(title)
    .bind(source_count)
    .bind(occurred_at)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn classify_revision(title: &str, is_new_story: bool, source_count: i32) -> &'static str {
    let normalized = title.to_lowercase();
    if ["correction", "corrected", "修正", "更正", "纠正"]
        .iter()
        .any(|marker| normalized.contains(marker))
    {
        "CORRECT"
    } else if title_signals_claim_contradiction(&normalized) {
        "CONTRADICT"
    } else if is_new_story {
        "ADD"
    } else if source_count >= 2 {
        "CONFIRM"
    } else {
        "UPDATE"
    }
}

fn title_signals_claim_contradiction(normalized_title: &str) -> bool {
    [
        "fact check",
        "fact-check",
        "false claim",
        "false report",
        "denies claim",
        "denies the claim",
        "denied claim",
        "denied the claim",
        "denies report",
        "denies the report",
        "denied report",
        "denied the report",
        "denies allegation",
        "denied allegation",
        "refutes claim",
        "refutes report",
        "debunks claim",
        "debunks report",
        "官方否认",
        "否认相关报道",
        "否认报道",
        "否认指控",
        "否认说法",
        "否认传闻",
        "辟谣",
        "报道不实",
        "消息不实",
        "说法不实",
        "传闻不实",
    ]
    .iter()
    .any(|marker| normalized_title.contains(marker))
}

async fn enabled_watches(state: &AppState) -> AppResult<Vec<Watch>> {
    Ok(all_watches(state)
        .await?
        .into_iter()
        .filter(|watch| watch.enabled)
        .collect())
}

async fn all_watches(state: &AppState) -> AppResult<Vec<Watch>> {
    let rows = sqlx::query(
        "SELECT id, name, query, min_impact, notification_mode, enabled, created_at, updated_at
         FROM watches
         ORDER BY enabled DESC, lower(name)",
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(rows.into_iter().map(watch_from_row).collect())
}

fn watch_from_row(row: sqlx::postgres::PgRow) -> Watch {
    Watch {
        id: row.get("id"),
        name: row.get("name"),
        query: row.get("query"),
        min_impact: row.get("min_impact"),
        notification_mode: row.get("notification_mode"),
        enabled: row.get("enabled"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

fn validate_watch(body: &WatchRequest) -> AppResult<()> {
    if body.name.trim().is_empty() || body.name.trim().chars().count() > 80 {
        return Err(AppError::BadRequest(
            "watch name must contain 1-80 characters".to_string(),
        ));
    }
    if body.query.trim().chars().count() < 2 || body.query.trim().chars().count() > 500 {
        return Err(AppError::BadRequest(
            "watch query must contain 2-500 characters".to_string(),
        ));
    }
    if !(0..=100).contains(&body.min_impact) {
        return Err(AppError::BadRequest(
            "min_impact must be between 0 and 100".to_string(),
        ));
    }
    if !matches!(body.notification_mode.as_str(), "off" | "brief" | "urgent") {
        return Err(AppError::BadRequest(
            "notification_mode must be off, brief or urgent".to_string(),
        ));
    }
    Ok(())
}

fn normalize_budget(minutes: i32) -> i32 {
    match minutes {
        0..=5 => 3,
        6..=14 => 10,
        _ => 20,
    }
}

fn budget_limit(minutes: i32) -> usize {
    match minutes {
        3 => 5,
        10 => 12,
        _ => 20,
    }
}

fn sorted_unique(mut values: Vec<String>) -> Vec<String> {
    values.sort_by_key(|value| value.to_lowercase());
    values.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    values
}

fn plain_excerpt(value: &str, max_chars: usize) -> String {
    let mut plain = String::with_capacity(value.len().min(max_chars * 2));
    let mut in_tag = false;
    for character in value.chars() {
        match character {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                plain.push(' ');
            }
            _ if !in_tag => plain.push(character),
            _ => {}
        }
    }
    let normalized = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_chars {
        normalized
    } else {
        format!(
            "{}…",
            normalized.chars().take(max_chars).collect::<String>()
        )
    }
}

fn default_min_impact() -> i32 {
    40
}

fn default_notification_mode() -> String {
    "brief".to_string()
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_brief_item(
        title: &str,
        total_score: i32,
        impact_score: i32,
        confidence_score: i32,
        source_quality_score: i32,
        independent_source_count: i32,
        source_title: &str,
    ) -> EventBriefItem {
        EventBriefItem {
            event_id: Uuid::new_v4(),
            revision_type: "ADD".to_string(),
            title: title.to_string(),
            what_happened: title.to_string(),
            why_it_matters: "test".to_string(),
            why_now: "test".to_string(),
            confidence_label: "test".to_string(),
            confidence_score,
            source_quality_score,
            impact_score,
            novelty_score: 80,
            urgency_score: 80,
            relevance_score: 35,
            total_score,
            source_count: independent_source_count.max(1),
            independent_source_count,
            source_titles: vec![source_title.to_string()],
            watch_ids: Vec::new(),
            watch_names: Vec::new(),
            representative_article_id: Uuid::new_v4(),
            published_at: Utc::now(),
            update_count: 1,
            has_grounded_summary: false,
            feedback: None,
            community_only: false,
            content_hash: Uuid::new_v4().to_string(),
            url_hash: Uuid::new_v4().to_string(),
        }
    }

    #[test]
    fn budgets_are_bounded_to_product_modes() {
        assert_eq!(normalize_budget(1), 3);
        assert_eq!(normalize_budget(9), 10);
        assert_eq!(normalize_budget(100), 20);
        assert_eq!(budget_limit(3), 5);
        assert_eq!(budget_limit(10), 12);
        assert_eq!(budget_limit(20), 20);
    }

    #[test]
    fn revisions_distinguish_corrections_and_contradictions() {
        assert_eq!(
            classify_revision("Correction: total revised", false, 1),
            "CORRECT"
        );
        assert_eq!(
            classify_revision("官方否认相关报道", false, 2),
            "CONTRADICT"
        );
        assert_eq!(classify_revision("A new event", true, 1), "ADD");
        assert_eq!(classify_revision("Another source", false, 3), "CONFIRM");
        assert_eq!(classify_revision("More details", false, 1), "UPDATE");
        assert_eq!(
            classify_revision("Appeals court denies Trump's request", false, 2),
            "CONFIRM"
        );
        assert_eq!(
            classify_revision("Late VAR drama denied equaliser", false, 1),
            "UPDATE"
        );
        assert_eq!(
            classify_revision(
                "The World Cup and the contradictions of national identity",
                false,
                2,
            ),
            "CONFIRM"
        );
        assert_eq!(
            classify_revision("Fact check: the viral report is false", false, 2),
            "CONTRADICT"
        );
    }

    #[test]
    fn excerpts_remove_markup_and_bound_length() {
        assert_eq!(
            plain_excerpt("<p>Hello <b>world</b></p>", 30),
            "Hello world"
        );
        assert_eq!(plain_excerpt("abcdef", 3), "abc…");
    }

    #[test]
    fn watch_queries_become_matchable_terms() {
        assert_eq!(
            watch_terms("OpenAI, AI Agent\n芯片"),
            vec!["openai", "ai", "agent", "芯片"]
        );
        assert!(!watch_query_matches(
            "the chair said the trail remains open",
            &watch_terms("AI Agent")
        ));
        assert!(watch_query_matches(
            "a new ai agent shipped today",
            &watch_terms("AI Agent")
        ));
        assert!(!watch_query_matches(
            "an unrelated agent was appointed",
            &watch_terms("AI Agent")
        ));
    }

    #[test]
    fn impact_signals_do_not_treat_ai_as_an_arbitrary_substring() {
        assert_eq!(impact_signal("Chair said plans remain unchanged", ""), 44);
        assert_eq!(topic_bucket("Chair said plans remain unchanged"), "general");
        assert_eq!(topic_bucket("New AI chip rules announced"), "technology");
        assert!(
            impact_signal("Military strikes close a major shipping route", "")
                > impact_signal("Championship final preview", "")
        );
    }

    #[test]
    fn brief_items_expose_persisted_feedback_state() {
        let item = score_candidate(
            Candidate {
                event_id: Uuid::nil(),
                title: "A verified event changed".to_string(),
                representative_article_id: Uuid::nil(),
                source_titles: vec!["Source".to_string()],
                source_count: 1,
                independent_source_count: 1,
                article_count: 1,
                update_count: 1,
                average_source_quality: 80,
                published_at: Utc::now(),
                summary: Some("A concise grounded summary".to_string()),
                summary_confidence: Some("high".to_string()),
                grounding_score: Some(90),
                excerpt: "A concise grounded summary".to_string(),
                revision_type: "UPDATE".to_string(),
                feedback: Some("known".to_string()),
                content_hash: "content-known".to_string(),
                url_hash: "url-known".to_string(),
            },
            &[],
            &[],
        );

        assert_eq!(item.feedback.as_deref(), Some("known"));
    }

    #[test]
    fn brief_dedupes_fragmented_breaking_news_titles() {
        assert!(titles_same_brief_event(
            "Live: US launches strikes after Iran fires on ship in Strait of Hormuz",
            "US launches fresh strikes as Iran closes Strait of Hormuz"
        ));
        assert!(!titles_same_brief_event(
            "US launches fresh strikes as Iran closes Strait of Hormuz",
            "UK to regulate casino sponsorship in football"
        ));
        assert!(titles_same_brief_event(
            "伊朗导弹袭击海湾油轮",
            "伊朗导弹再次袭击海湾地区油轮"
        ));
    }

    #[test]
    fn freshness_decays_continuously_from_latest_change() {
        let now = freshness_score(0.0, 0, "ADD");
        let twelve_hours = freshness_score(12.0, 0, "ADD");
        let one_day = freshness_score(24.0, 0, "ADD");
        assert_eq!(now, 100);
        assert_eq!(twelve_hours, 50);
        assert_eq!(one_day, 25);
        assert!(freshness_score(12.0, 2, "UPDATE") > twelve_hours);
    }

    #[test]
    fn substantive_changes_outweigh_repeated_articles() {
        assert!(change_novelty_score("CORRECT", 3, 1) > change_novelty_score("ADD", 1, 1));
        assert!(change_novelty_score("UPDATE", 2, 2) > change_novelty_score("ADD", 6, 0));
    }

    #[test]
    fn evidence_gate_drops_fresh_low_signal_noise() {
        let mut low_signal = test_brief_item("Fresh forum chatter", 67, 64, 45, 50, 1, "NodeSeek");
        low_signal.community_only = true;
        let trusted_single = test_brief_item("Trusted original report", 60, 44, 48, 80, 1, "Wire");
        let corroborated = test_brief_item("Corroborated event", 58, 44, 55, 55, 2, "Wire");
        let mut watched_low_signal = low_signal.clone();
        watched_low_signal.watch_ids.push(Uuid::new_v4());
        let mut rejected = test_brief_item("Explicitly irrelevant", 90, 90, 90, 90, 3, "Wire");
        rejected.feedback = Some("irrelevant".to_string());

        assert!(!passes_evidence_gate(&low_signal));
        assert!(!passes_evidence_gate(&watched_low_signal));
        assert!(passes_evidence_gate(&trusted_single));
        assert!(passes_evidence_gate(&corroborated));
        assert!(!passes_evidence_gate(&rejected));
    }

    #[test]
    fn community_sources_need_stronger_evidence() {
        assert!(sources_are_community_only(&[
            "NodeSeek".to_string(),
            "V2EX 技术".to_string()
        ]));
        assert!(!sources_are_community_only(&[
            "BBC News".to_string(),
            "NodeSeek".to_string()
        ]));
    }

    #[test]
    fn preference_feedback_changes_related_events_not_workflow_state() {
        let candidate = Candidate {
            event_id: Uuid::new_v4(),
            title: "AI chip export rules tighten again".to_string(),
            representative_article_id: Uuid::new_v4(),
            source_titles: vec!["World Wire".to_string()],
            source_count: 1,
            independent_source_count: 1,
            article_count: 1,
            update_count: 1,
            average_source_quality: 60,
            published_at: Utc::now(),
            summary: None,
            summary_confidence: None,
            grounding_score: None,
            excerpt: "New export controls affect advanced chips".to_string(),
            revision_type: "UPDATE".to_string(),
            feedback: None,
            content_hash: "preference-content".to_string(),
            url_hash: "preference-url".to_string(),
        };
        let related_more = PreferenceSignal {
            event_id: Uuid::new_v4(),
            kind: "more".to_string(),
            title: "New AI chip export rules announced".to_string(),
        };
        let unrelated_less = PreferenceSignal {
            event_id: Uuid::new_v4(),
            kind: "less".to_string(),
            title: "Football club wins cup final".to_string(),
        };
        let known = PreferenceSignal {
            event_id: Uuid::new_v4(),
            kind: "known".to_string(),
            title: "AI chip export rules".to_string(),
        };

        assert!(preference_delta(&candidate, &[related_more]) > 0);
        assert_eq!(preference_delta(&candidate, &[unrelated_less]), 0);
        assert_eq!(preference_delta(&candidate, &[known]), 0);
    }

    #[test]
    fn reranking_prefers_source_and_topic_coverage() {
        let first = test_brief_item(
            "AI chip export rules change",
            90,
            80,
            70,
            80,
            1,
            "Technology Wire",
        );
        let same_lane = test_brief_item(
            "Cloud software firms launch new AI products",
            89,
            80,
            70,
            80,
            1,
            "Technology Wire",
        );
        let broader = test_brief_item(
            "Election commission confirms national vote date",
            85,
            80,
            70,
            80,
            1,
            "World Wire",
        );

        let selected = select_diverse(vec![same_lane, broader.clone(), first.clone()], 2);
        assert_eq!(selected[0].event_id, first.event_id);
        assert_eq!(selected[1].event_id, broader.event_id);
    }

    #[test]
    fn exact_content_duplicates_never_consume_two_slots() {
        let first = test_brief_item("First rendering of an event", 90, 80, 70, 80, 1, "Wire A");
        let mut duplicate = test_brief_item(
            "Completely different title for the copied body",
            89,
            80,
            70,
            80,
            1,
            "Wire B",
        );
        duplicate.content_hash = first.content_hash.clone();
        let distinct = test_brief_item("A distinct event", 80, 70, 65, 80, 1, "Wire C");

        let selected = select_diverse(vec![duplicate, distinct.clone(), first.clone()], 2);
        assert_eq!(selected[0].event_id, first.event_id);
        assert_eq!(selected[1].event_id, distinct.event_id);
    }
}
