use std::collections::{HashMap, HashSet};

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
    config::EditorialConfig,
    error::{AppError, AppResult},
    extractor::clean_article_text,
    state::AppState,
};

mod evidence;
use evidence::{EventEvidence, EvidenceScope};

/// 只作安全边界：窗口过滤已限定范围。2026-10-20 现网 24 小时有 2,562 个事件簇，
/// 原上限 2,000 按最近更新截断，把 17 小时以前的事件全部挡在报告外。
const MAX_CANDIDATES: i64 = 5_000;
const MIN_BRIEF_SCORE: i32 = 60;

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(crate::osint::router())
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

#[derive(Debug, Deserialize)]
struct EventDetailQuery {
    representative_article_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
pub(crate) struct WireResponse {
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
pub(crate) struct BriefResponse {
    pub(crate) generated_at: DateTime<Utc>,
    pub(crate) window_hours: i64,
    mode: &'static str,
    budget_minutes: Option<i32>,
    pub(crate) stats: BriefStats,
    pub(crate) items: Vec<EventBriefItem>,
}

#[derive(Debug, Serialize)]
pub(crate) struct BriefStats {
    pub(crate) candidate_events: usize,
    eligible_events: usize,
    selected_events: usize,
    independent_sources: usize,
    watch_matches: usize,
    /// 已有 LLM 评审的候选数。
    assessed_events: usize,
}

/// 评审得出的“此前进展”引用。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryRef {
    pub event_id: Uuid,
    pub title: String,
    pub digest_date: String,
}

/// 编辑层评审结果，来自 `event_assessments`。
#[derive(Debug, Clone, Default)]
struct Assessment {
    profile: String,
    score: f32,
    reason: String,
    summary: String,
    tags: Vec<String>,
    history_note: Option<String>,
    history_refs: Vec<HistoryRef>,
    community_note: Option<String>,
    takeaway: Option<String>,
    section: Option<String>,
    headline: Option<String>,
}

/// 等待 LLM 评审的候选事件。
#[derive(Debug, Clone)]
pub(crate) struct AssessmentCandidate {
    pub(crate) event_id: Uuid,
    pub(crate) materials_key: String,
    pub(crate) priority: i32,
    pub(crate) independent_source_count: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct EventBriefItem {
    pub(crate) event_id: Uuid,
    pub(crate) report_section: String,
    revision_type: String,
    pub(crate) title: String,
    pub(crate) what_happened: String,
    pub(crate) why_it_matters: String,
    why_now: String,
    pub(crate) confidence_label: String,
    confidence_score: i32,
    source_quality_score: i32,
    impact_score: i32,
    novelty_score: i32,
    urgency_score: i32,
    relevance_score: i32,
    total_score: i32,
    source_count: i32,
    pub(crate) independent_source_count: i32,
    pub(crate) source_titles: Vec<String>,
    watch_ids: Vec<Uuid>,
    watch_names: Vec<String>,
    pub(crate) representative_article_id: Uuid,
    pub(crate) published_at: DateTime<Utc>,
    update_count: i32,
    has_grounded_summary: bool,
    feedback: Option<String>,
    /// LLM 评审的 Profile 与分数（0–10）；未评审时为空。
    pub(crate) editorial_profile: Option<String>,
    pub(crate) editorial_score: Option<f32>,
    pub(crate) editorial_tags: Vec<String>,
    pub(crate) history_note: Option<String>,
    pub(crate) history_refs: Vec<HistoryRef>,
    pub(crate) community_note: Option<String>,
    pub(crate) takeaway: Option<String>,
    #[serde(skip_serializing)]
    below_editorial_threshold: bool,
    #[serde(skip_serializing)]
    awaiting_assessment: bool,
    #[serde(skip_serializing)]
    source_families: Vec<String>,
    #[serde(skip_serializing)]
    community_only: bool,
    #[serde(skip_serializing)]
    content_hash: String,
    #[serde(skip_serializing)]
    url_hash: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct EventDetail {
    event: EventBriefItem,
    timeline: Vec<EventRevision>,
    evidence: Vec<EventEvidence>,
    evidence_scope: EvidenceScope,
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

#[derive(Debug, Clone)]
struct Candidate {
    event_id: Uuid,
    title: String,
    representative_article_id: Uuid,
    source_titles: Vec<String>,
    source_families: Vec<String>,
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
    assessment: Option<Assessment>,
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
    let budget_minutes = query.minutes.map(normalize_budget);
    let report_mode = budget_minutes.is_none();
    let hours = if report_mode {
        normalize_report_hours(query.hours.unwrap_or(12))
    } else {
        query.hours.unwrap_or(24).clamp(1, 24 * 30)
    };
    let limit = budget_minutes.map_or_else(|| report_limit(hours), budget_limit);
    Ok(Json(build_brief(&state, hours, limit, budget_minutes).await?))
}

/// 生成简报：评分、证据门槛、多样性重排和栏目配额。日报与 MCP 共用。
pub(crate) async fn build_brief(
    state: &AppState,
    hours: i64,
    limit: usize,
    budget_minutes: Option<i32>,
) -> AppResult<BriefResponse> {
    let report_mode = budget_minutes.is_none();
    let editorial = &state.config.editorial;
    let watches = enabled_watches(state).await?;
    let preferences = load_preference_signals(state).await?;
    let candidates = load_candidates(state, hours, None).await?;
    let candidate_count = candidates.len();
    let assessed_events = candidates
        .iter()
        .filter(|candidate| candidate.assessment.is_some())
        .count();
    let ranked = candidates
        .into_iter()
        .map(|candidate| score_candidate(candidate, &watches, &preferences, editorial))
        .collect::<Vec<_>>();
    let eligible_events = ranked
        .iter()
        .filter(|item| passes_evidence_gate(item))
        .count();
    let items = select_diverse(ranked, limit, &editorial.section_shares);
    let independent_sources = items
        .iter()
        .flat_map(|item| item.source_titles.iter().cloned())
        .collect::<HashSet<_>>()
        .len();
    let watch_matches = items
        .iter()
        .filter(|item| !item.watch_ids.is_empty())
        .count();

    Ok(BriefResponse {
        generated_at: Utc::now(),
        window_hours: hours,
        mode: if report_mode { "report" } else { "brief" },
        budget_minutes,
        stats: BriefStats {
            candidate_events: candidate_count,
            eligible_events,
            selected_events: items.len(),
            independent_sources,
            watch_matches,
            assessed_events,
        },
        items,
    })
}

/// 列出值得 LLM 评审的候选：按不含评审的规则分数做宽松预筛，优先多源与命中关注的事件。
pub(crate) async fn assessment_candidates(
    state: &AppState,
    hours: i64,
) -> AppResult<Vec<AssessmentCandidate>> {
    let watches = enabled_watches(state).await?;
    let preferences = load_preference_signals(state).await?;
    let editorial = &state.config.editorial;
    let mut queue = load_candidates(state, hours, None)
        .await?
        .into_iter()
        .filter_map(|mut candidate| {
            candidate.assessment = None;
            // 来源数跨档或出现纠正/矛盾时才重新评审；代表文章变化不触发，避免反复计费。
            let materials_key = format!(
                "sources:{}:{}",
                source_count_bucket(candidate.independent_source_count),
                if matches!(candidate.revision_type.as_str(), "CORRECT" | "CONTRADICT") {
                    "changed"
                } else {
                    "stable"
                }
            );
            let item = score_candidate(candidate, &watches, &preferences, editorial);
            passes_assessment_pregate(&item).then(|| AssessmentCandidate {
                event_id: item.event_id,
                materials_key,
                priority: item.total_score
                    + if item.watch_ids.is_empty() { 0 } else { 15 }
                    + (item.independent_source_count.min(4) - 1).max(0) * 5,
                independent_source_count: item.independent_source_count,
            })
        })
        .collect::<Vec<_>>();
    queue.sort_by(|left, right| right.priority.cmp(&left.priority));
    Ok(queue)
}

/// 预筛比简报证据门槛宽：评审本身负责把关键词分漏掉的重要事件提上来。
fn passes_assessment_pregate(item: &EventBriefItem) -> bool {
    if matches!(item.feedback.as_deref(), Some("irrelevant" | "bad_cluster")) {
        return false;
    }
    !item.watch_ids.is_empty()
        || item.independent_source_count >= 2
        || item.source_quality_score >= 70
        || (!item.community_only && item.total_score >= 50)
}

fn source_count_bucket(count: i32) -> &'static str {
    match count {
        ..=1 => "1",
        2 => "2",
        3..=4 => "3-4",
        _ => "5+",
    }
}

async fn wire_now(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<WireQuery>,
) -> AppResult<Json<WireResponse>> {
    auth::require_session(&state.pool, &headers).await?;
    let hours = query.hours.unwrap_or(24).clamp(1, 24 * 30);
    let limit = query.limit.unwrap_or(30).clamp(1, 100);
    Ok(Json(load_wire(&state, hours, limit).await?))
}

/// 已发布的事件快讯。MCP 共用。
pub(crate) async fn load_wire(state: &AppState, hours: i64, limit: i64) -> AppResult<WireResponse> {
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
    Ok(WireResponse {
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
    })
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
    Query(query): Query<EventDetailQuery>,
) -> AppResult<Json<EventDetail>> {
    auth::require_session(&state.pool, &headers).await?;
    Ok(Json(
        load_event_detail(&state, id, query.representative_article_id).await?,
    ))
}

/// 事件详情：评分、时间线和证据。事件被合并时按代表文章找到当前事件。MCP 共用。
pub(crate) async fn load_event_detail(
    state: &AppState,
    id: Uuid,
    representative_article_id: Option<Uuid>,
) -> AppResult<EventDetail> {
    let watches = enabled_watches(state).await?;
    let preferences = load_preference_signals(state).await?;
    let candidate = match load_candidates(state, 24 * 365, Some(id))
        .await?
        .into_iter()
        .next()
    {
        Some(candidate) => candidate,
        None => {
            let article_id = representative_article_id.ok_or(AppError::NotFound)?;
            let current_id = sqlx::query_scalar::<_, Uuid>(
                "SELECT story_id FROM articles WHERE id = $1 AND story_id IS NOT NULL",
            )
            .bind(article_id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or(AppError::NotFound)?;
            if current_id == id {
                return Err(AppError::NotFound);
            }
            load_candidates(state, 24 * 365, Some(current_id))
                .await?
                .into_iter()
                .next()
                .ok_or(AppError::NotFound)?
        }
    };
    let id = candidate.event_id;
    let event = score_candidate(candidate, &watches, &preferences, &state.config.editorial);
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
    let (evidence, evidence_scope) = evidence::load(&state.pool, id).await?;

    Ok(EventDetail {
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
        evidence,
        evidence_scope,
    })
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
         visible_articles AS (
           SELECT a.id, a.story_id, a.published_at, a.fetched_at, f.title AS source_title,
                  LEAST(100, GREATEST(0,
                    f.quality_score + COALESCE(sf.weight, 0) + COALESCE(fqs.auto_adjust, 0)
                  )) AS source_quality,
                  CASE
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
                  END AS source_family
           FROM recent_stories sc
           JOIN articles a ON a.story_id = sc.id
           JOIN feeds f ON f.id = a.feed_id
           LEFT JOIN source_feedback sf ON sf.feed_id = f.id
           LEFT JOIN feed_quality_stats fqs ON fqs.feed_id = f.id
           WHERE COALESCE(sf.muted, false) = false
             AND ($1::uuid IS NOT NULL OR COALESCE(a.published_at, a.fetched_at) >= now() - ($2 || ' hours')::interval)
         ),
         publisher_quality AS (
           SELECT story_id, source_family, MAX(source_quality) AS quality
           FROM visible_articles
           GROUP BY story_id, source_family
         ),
         story_signals AS (
           SELECT sc.id, sc.representative_title,
                  GREATEST(sc.source_count, 1) AS source_count,
                  COUNT(a.id)::integer AS article_count,
                  COUNT(DISTINCT a.source_family)::integer AS independent_source_count,
                  COUNT(*) FILTER (
                    WHERE COALESCE(a.published_at, a.fetched_at) >= now() - interval '6 hours'
                  )::integer AS update_count,
                  ARRAY_AGG(DISTINCT a.source_title) AS source_titles,
                  ARRAY_AGG(DISTINCT a.source_family) AS source_families,
                  MAX(COALESCE(a.published_at, a.fetched_at)) AS published_at
           FROM recent_stories sc
           JOIN visible_articles a ON a.story_id = sc.id
           GROUP BY sc.id, sc.representative_title, sc.source_count
         )
         SELECT ss.*, sq.average_source_quality,
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
                ef.kind AS feedback,
                ea.profile AS ea_profile, ea.score AS ea_score, ea.reason AS ea_reason,
                ea.summary AS ea_summary, ea.tags AS ea_tags,
                ea.history_note AS ea_history_note, ea.history_refs AS ea_history_refs,
                ea.community_note AS ea_community_note, ea.takeaway AS ea_takeaway,
                ea.section AS ea_section, ea.headline AS ea_headline
         FROM story_signals ss
         JOIN (
           SELECT story_id, ROUND(AVG(quality))::integer AS average_source_quality
           FROM publisher_quality
           GROUP BY story_id
         ) sq ON sq.story_id = ss.id
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
         LEFT JOIN event_assessments ea
           ON ea.story_id = ss.id AND ea.status = 'done' AND ea.score IS NOT NULL
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
                source_families: row.get("source_families"),
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
                assessment: assessment_from_row(&row),
            }
        })
        .collect())
}

fn assessment_from_row(row: &sqlx::postgres::PgRow) -> Option<Assessment> {
    let score = row.get::<Option<f32>, _>("ea_score")?;
    let non_empty = |value: Option<String>| value.filter(|value| !value.trim().is_empty());
    Some(Assessment {
        profile: row.get::<Option<String>, _>("ea_profile").unwrap_or_default(),
        score,
        reason: row.get::<Option<String>, _>("ea_reason").unwrap_or_default(),
        summary: row.get::<Option<String>, _>("ea_summary").unwrap_or_default(),
        tags: row
            .get::<Option<serde_json::Value>, _>("ea_tags")
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default(),
        history_note: non_empty(row.get("ea_history_note")),
        history_refs: row
            .get::<Option<serde_json::Value>, _>("ea_history_refs")
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default(),
        community_note: non_empty(row.get("ea_community_note")),
        takeaway: non_empty(row.get("ea_takeaway")),
        section: non_empty(row.get("ea_section")),
        headline: non_empty(row.get("ea_headline")),
    })
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
    editorial: &EditorialConfig,
) -> EventBriefItem {
    let age_hours = (Utc::now() - candidate.published_at).num_minutes().max(0) as f32 / 60.0;
    let urgency = freshness_score(age_hours, candidate.update_count, &candidate.revision_type);
    let novelty = change_novelty_score(
        &candidate.revision_type,
        candidate.article_count,
        candidate.update_count,
    );
    let keyword_impact = impact_signal(&candidate.title, &candidate.excerpt);
    // 有 LLM 评审时以评审分为准；关键词分只在评审缺失时兜底。
    let impact = candidate
        .assessment
        .as_ref()
        .map_or(keyword_impact, |assessment| {
            (assessment.score * 10.0).round().clamp(0.0, 100.0) as i32
        });
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
    // 影响分来自评审时更可信，权重从 0.28 提到 0.36，时效从 0.18 降到 0.10。
    // 2026-10-20 回放 650 个已评审事件：入选平均评审分 6.28→6.52，≤5.5 分条目 31→17。
    let total = (impact as f32 * 0.36
        + novelty.clamp(0, 100) as f32 * 0.20
        + urgency.clamp(0, 100) as f32 * 0.10
        + confidence as f32 * 0.19
        + relevance as f32 * 0.15)
        .round() as i32;
    let has_grounded_summary = candidate.grounding_score.is_some()
        && candidate
            .summary
            .as_ref()
            .is_some_and(|summary| !summary.trim().is_empty());
    let assessment = candidate.assessment.clone().unwrap_or_default();
    let assessed = candidate.assessment.is_some();
    let what_happened = candidate
        .summary
        .filter(|summary| !summary.trim().is_empty())
        .or_else(|| (assessed && !assessment.summary.trim().is_empty()).then(|| assessment.summary.clone()))
        .unwrap_or_else(|| candidate.excerpt.clone());
    let why_it_matters = if assessed && !assessment.reason.trim().is_empty() {
        match watch_names.first() {
            Some(watch) => format!("命中关注「{watch}」。{}", assessment.reason),
            None => assessment.reason.clone(),
        }
    } else {
        importance_reason(
            impact,
            candidate.independent_source_count,
            candidate.average_source_quality,
            &watch_names,
        )
    };
    let below_editorial_threshold =
        assessed && assessment.score < editorial.threshold(&assessment.profile);
    let why_now = change_reason(&candidate.revision_type, candidate.update_count, age_hours);
    let confidence_label = confidence_label(
        confidence,
        candidate.independent_source_count,
        candidate.summary_confidence.as_deref(),
    );
    // 评审给出的栏目按事件实质判断，优先于关键词；旧评审没有栏目时回退关键词。
    let report_section = assessment
        .section
        .as_deref()
        .and_then(|section| REPORT_SECTIONS.iter().find(|(key, _)| *key == section))
        .map_or_else(|| report_section(&candidate.title, &what_happened), |(key, _)| *key);
    // 频道帖子的原文标题（表情前缀、“JUST IN:”、整段正文）换成评审写的短标题；正常标题保持原文。
    let title = match assessment.headline.as_deref() {
        Some(headline) if title_is_noisy(&candidate.title) => headline.to_string(),
        _ => candidate.title,
    };
    // 评审开启时，单源事件要先过评审才进简报：评审名额用完后，未评审的单源条目只有关键词分，
    // 实测会把“今天没什么新闻”、二手转让、招展广告带进报告后半段。多源事件和命中关注的不受影响。
    // 只有论坛来源的事件同理：同一帖子跨站转发会被算成两个独立来源。
    let community_only = sources_are_community_only(&candidate.source_families);
    let awaiting_assessment = editorial.enabled
        && !assessed
        && (candidate.independent_source_count < 2 || community_only)
        && watch_ids.is_empty();

    EventBriefItem {
        event_id: candidate.event_id,
        report_section: report_section.to_string(),
        revision_type: candidate.revision_type,
        title,
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
        source_families: candidate.source_families,
        watch_ids,
        watch_names,
        representative_article_id: candidate.representative_article_id,
        published_at: candidate.published_at,
        update_count: candidate.update_count,
        has_grounded_summary,
        feedback: candidate.feedback,
        editorial_profile: assessed.then(|| assessment.profile.clone()),
        editorial_score: assessed.then_some(assessment.score),
        editorial_tags: assessment.tags,
        history_note: assessment.history_note,
        history_refs: assessment.history_refs,
        community_note: assessment.community_note,
        takeaway: assessment.takeaway,
        below_editorial_threshold,
        awaiting_assessment,
        community_only,
        content_hash: candidate.content_hash,
        url_hash: candidate.url_hash,
    }
}

fn select_diverse(
    items: Vec<EventBriefItem>,
    limit: usize,
    section_shares: &HashMap<String, f32>,
) -> Vec<EventBriefItem> {
    select_diverse_traced(items, limit, section_shares, &mut |_, _| {})
}

/// `on_drop` 收到每个落选条目和原因，供 `explain-brief` 排查“为什么没进报告”。
fn select_diverse_traced(
    mut items: Vec<EventBriefItem>,
    limit: usize,
    section_shares: &HashMap<String, f32>,
    on_drop: &mut dyn FnMut(&EventBriefItem, String),
) -> Vec<EventBriefItem> {
    items.retain(|item| {
        let pass = passes_evidence_gate(item);
        if !pass {
            on_drop(item, "未过证据门槛".to_string());
        }
        pass
    });
    // 标题只分词一次；与已选条目的最大相似度和重复判断随每次入选增量更新，
    // 每轮只和新入选的一条比较。原实现每轮对全部候选 × 全部已选重新分词，
    // 2026-10-20 现网约 1,900 个候选时简报接口耗时 14 秒。
    let mut pool = items
        .into_iter()
        .map(|item| {
            let tokens = brief_title_tokens(&item.title);
            (item, tokens, 0.0_f32)
        })
        .collect::<Vec<_>>();
    let mut selected = Vec::<EventBriefItem>::with_capacity(limit);
    let mut section_counts = HashMap::<String, usize>::new();
    while !pool.is_empty() && selected.len() < limit {
        pool.retain(|(item, _, _)| {
            let open = section_cap(&item.report_section, limit, section_shares).is_none_or(|cap| {
                section_counts.get(&item.report_section).copied().unwrap_or(0) < cap
            });
            if !open {
                on_drop(item, format!("栏目 {} 已满（第 {} 轮）", item.report_section, selected.len()));
            }
            open
        });
        let Some(best_index) = pool
            .iter()
            .map(|(item, _, max_similarity)| {
                (selection_score_with(item, &selected, limit, *max_similarity), item)
            })
            .enumerate()
            .max_by(|(_, (left_score, left)), (_, (right_score, right))| {
                left_score
                    .total_cmp(right_score)
                    .then_with(|| left.total_score.cmp(&right.total_score))
                    .then_with(|| left.published_at.cmp(&right.published_at))
            })
            .map(|(index, _)| index)
        else {
            break;
        };
        let (item, tokens, _) = pool.swap_remove(best_index);
        pool.retain_mut(|(candidate, candidate_tokens, max_similarity)| {
            let reason = if candidate.content_hash == item.content_hash {
                Some("正文哈希相同")
            } else if candidate.url_hash == item.url_hash {
                Some("链接哈希相同")
            } else if token_sets_same_event(&tokens, candidate_tokens) {
                Some("标题判为同一事件")
            } else {
                None
            };
            if let Some(reason) = reason {
                on_drop(candidate, format!("{reason}：{}", item.title));
                return false;
            }
            *max_similarity = max_similarity.max(token_set_similarity(&tokens, candidate_tokens));
            true
        });
        *section_counts.entry(item.report_section.clone()).or_default() += 1;
        selected.push(item);
    }
    for (item, _, max_similarity) in &pool {
        on_drop(
            item,
            format!(
                "名额已满，最终挑选分 {:.1}",
                selection_score_with(item, &selected, limit, *max_similarity)
            ),
        );
    }
    selected
}

/// 运维排查：给定事件为什么进或没进 `hours` 小时报告。只读。
pub(crate) async fn explain_brief(state: &AppState, hours: i64, ids: &[Uuid]) -> AppResult<Vec<String>> {
    let editorial = &state.config.editorial;
    let watches = enabled_watches(state).await?;
    let preferences = load_preference_signals(state).await?;
    let ranked = load_candidates(state, hours, None)
        .await?
        .into_iter()
        .map(|candidate| score_candidate(candidate, &watches, &preferences, editorial))
        .collect::<Vec<_>>();
    let mut lines = vec![format!("候选 {} 个", ranked.len())];
    let wanted = ids.iter().copied().collect::<HashSet<_>>();
    for item in ranked.iter().filter(|item| wanted.contains(&item.event_id)) {
        lines.push(format!(
            "{} 总分 {} 评审 {:?} 栏目 {} 独立来源 {} | {}",
            item.event_id, item.total_score, item.editorial_score, item.report_section,
            item.independent_source_count, item.title
        ));
    }
    for id in ids.iter().filter(|id| !ranked.iter().any(|item| item.event_id == **id)) {
        lines.push(format!("{id} 不在候选集"));
    }
    let mut drops = Vec::new();
    let selected = select_diverse_traced(
        ranked,
        report_limit(hours),
        &editorial.section_shares,
        &mut |item, reason| {
            if wanted.contains(&item.event_id) {
                drops.push(format!("{} 落选：{reason}", item.event_id));
            }
        },
    );
    lines.extend(drops);
    for (rank, item) in selected.iter().enumerate().filter(|(_, item)| wanted.contains(&item.event_id)) {
        lines.push(format!("{} 入选第 {}", item.event_id, rank + 1));
    }
    Ok(lines)
}

/// 栏目硬上限：留下刚好够读的量，让热门栏目给其他栏目留位置。未配置的栏目不设上限。
fn section_cap(section: &str, limit: usize, shares: &HashMap<String, f32>) -> Option<usize> {
    let share = *shares.get(section)?;
    Some(((limit as f32 * share).ceil() as usize).max(1))
}

fn passes_evidence_gate(item: &EventBriefItem) -> bool {
    if matches!(item.feedback.as_deref(), Some("irrelevant" | "bad_cluster")) {
        return false;
    }
    // 评审判为低于 Profile 阈值的事件不进简报；命中关注的事件仍按原规则判断。
    if item.below_editorial_threshold && item.watch_ids.is_empty() {
        return false;
    }
    if item.awaiting_assessment {
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

#[cfg(test)]
fn selection_score(item: &EventBriefItem, selected: &[EventBriefItem], limit: usize) -> f32 {
    let max_similarity = selected
        .iter()
        .map(|selected_item| brief_title_similarity(&selected_item.title, &item.title))
        .fold(0.0_f32, f32::max);
    selection_score_with(item, selected, limit, max_similarity)
}

/// `max_similarity` 是条目标题与已选条目标题的最大 Jaccard 相似度，由调用方增量维护。
fn selection_score_with(
    item: &EventBriefItem,
    selected: &[EventBriefItem],
    limit: usize,
    max_similarity: f32,
) -> f32 {
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

    score -= max_similarity * 22.0;

    let topic = item.report_section.as_str();
    let topic_count = selected
        .iter()
        .filter(|selected_item| selected_item.report_section == topic)
        .count();
    // 栏目已细分，软上限按期数放宽；硬上限另由 section_shares 控制。
    let topic_cap = if topic == "general" {
        (limit / 3).max(2)
    } else {
        (limit / 10).max(2)
    };
    if topic_count >= topic_cap {
        score -= ((topic_count - topic_cap + 1) as f32) * 10.0;
    }

    let normalized_sources = item
        .source_families
        .iter()
        .map(|source| source.trim().to_lowercase())
        .collect::<HashSet<_>>();
    // 来源重复扣分只针对一两个来源撑起的条目，防止同一信源刷屏。
    // 多源事件和已选条目共享大通讯社是常态，按重复扣分会把最重要的事件压到单源快讯后面。
    if !normalized_sources.is_empty() && normalized_sources.len() <= 2 {
        let repeat_counts = normalized_sources
            .iter()
            .map(|source| {
                selected
                    .iter()
                    .filter(|selected_item| {
                        selected_item.source_families.iter().any(|selected_source| {
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

/// 原文标题是否像频道帖子而不是新闻标题：以表情或括号前缀开头、带“JUST IN”一类口播前缀、
/// 换行，或长到明显是整段正文。
fn title_is_noisy(title: &str) -> bool {
    let title = title.trim();
    let Some(first) = title.chars().next() else {
        return false;
    };
    if title.contains('\n') || !(first.is_alphanumeric() || "‘“'\"«".contains(first)) {
        return true;
    }
    let upper = title.to_uppercase();
    if ["JUST IN", "BREAKING", "WOW."].iter().any(|prefix| upper.starts_with(prefix)) {
        return true;
    }
    let length = title.chars().count();
    if title.chars().any(is_cjk) { length > 60 } else { length > 140 }
}

#[cfg(test)]
fn titles_same_brief_event(left: &str, right: &str) -> bool {
    token_sets_same_event(&brief_title_tokens(left), &brief_title_tokens(right))
}

fn token_sets_same_event(left: &HashSet<String>, right: &HashSet<String>) -> bool {
    if left.len() < 3 || right.len() < 3 {
        return false;
    }
    let shared = left.intersection(right).count();
    let union = left.union(right).count();
    shared >= 3 && shared as f32 / union.max(1) as f32 >= 0.20
}

fn brief_title_similarity(left: &str, right: &str) -> f32 {
    token_set_similarity(&brief_title_tokens(left), &brief_title_tokens(right))
}

fn token_set_similarity(left: &HashSet<String>, right: &HashSet<String>) -> f32 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    left.intersection(right).count() as f32 / left.union(right).count().max(1) as f32
}

fn preference_delta(candidate: &Candidate, preferences: &[PreferenceSignal]) -> i32 {
    let mut strongest_positive = 0;
    let mut strongest_negative = 0;
    let candidate_topic = report_section(&candidate.title, &candidate.excerpt);
    for signal in preferences {
        if signal.event_id == candidate.event_id {
            continue;
        }
        let title_similarity = brief_title_similarity(&candidate.title, &signal.title);
        if title_similarity <= 0.0 {
            continue;
        }
        let same_topic = report_section(&signal.title, "") == candidate_topic;
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

fn sources_are_community_only(source_families: &[String]) -> bool {
    !source_families.is_empty()
        && source_families.iter().all(|source| {
            let normalized = source.trim().to_ascii_lowercase();
            if let Some(route) = normalized.strip_prefix("rsshub:") {
                return matches!(
                    route.split('/').next(),
                    Some("v2ex" | "nodeseek" | "linuxdo")
                );
            }
            ["v2ex.com", "nodeseek.com", "linux.do"]
                .iter()
                .any(|domain| normalized == *domain || normalized.ends_with(&format!(".{domain}")))
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
    // 半衰期等于全天报告的窗口：12 小时半衰期时，18 小时前的多源大事（7.8 分、13 源）
    // 总分低于刚发布的 5 分单源快讯（2026-10-20 回放）。
    let continuous_decay = 100.0 * 2.0_f32.powf(-age_hours.max(0.0) / 24.0);
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

/// 报告栏目，顺序即展示顺序，也是同分时的优先顺序。前端和日报使用同一组键。
pub(crate) const REPORT_SECTIONS: &[(&str, &str)] = &[
    ("conflict", "战争与冲突"),
    ("politics", "政治与外交"),
    ("society", "社会与民生"),
    ("disaster", "灾害与事故"),
    ("economy", "宏观与市场"),
    ("business", "公司与产业"),
    ("ai", "AI"),
    ("tech", "科技与产品"),
    ("cyber", "网络安全"),
    ("science", "科学与健康"),
    ("crypto", "加密资产"),
    ("culture", "文化与体育"),
    ("general", "其他"),
];

/// 各栏目的信号词。英文按词匹配并接受常见词尾（riots、rioting、curfews），中文按子串匹配。
/// 词有歧义时宁可不收：例如 gas（催泪瓦斯）、who（疑问词）、币（人民币）。
const SECTION_MARKERS: &[(&str, &[&str])] = &[
    ("conflict", &[
        "war", "warfare", "airstrike", "missile", "drone", "troop", "military", "army",
        "soldier", "ceasefire", "truce", "shelling", "invasion", "invade", "offensive",
        "militant", "hamas", "hezbollah", "houthi", "idf", "nato", "gaza", "frontline",
        "combat", "bombing", "artillery", "insurgent", "rebel", "weapon", "strike", "attack",
        "defence", "defense",
        "战争", "战事", "空袭", "袭击", "导弹", "无人机", "军事", "军方", "部队", "士兵", "停火",
        "炮击", "入侵", "武装", "前线", "交火", "轰炸", "胡塞", "哈马斯", "真主党", "北约", "加沙",
        "以军", "俄军", "乌军",
    ]),
    ("politics", &[
        "election", "vote", "voting", "voter", "ballot", "parliament", "president",
        "presidential", "prime", "minister", "government", "cabinet", "diplomat", "diplomatic",
        "diplomacy", "summit", "sanction", "senate", "congress", "lawmaker", "legislation",
        "court", "judge", "ruling", "supreme", "kremlin", "trump", "putin", "zelensky",
        "macron", "netanyahu", "republican", "democrat", "opposition", "coup", "referendum",
        "treaty", "ambassador", "envoy", "party", "mayor", "governor", "midterm",
        "brexit", "eu", "governance",
        "选举", "大选", "投票", "议会", "国会", "总统", "总理", "首相", "政府", "内阁", "外交",
        "峰会", "制裁", "部长", "法院", "裁决", "政党", "公投", "条约", "大使", "特使", "白宫",
        "克里姆林宫", "特朗普", "普京", "泽连斯基", "马克龙", "内塔尼亚胡", "欧盟", "联合国",
        "外长", "会晤", "谈判", "市长", "州长", "中期选举",
    ]),
    ("society", &[
        "protest", "protester", "riot", "rioter", "unrest", "curfew", "loot", "demonstration",
        "demonstrator", "walkout", "migrant", "migration", "immigration", "refugee", "asylum",
        "crime", "shooting", "shooter", "gunman", "police", "arrest", "prison", "hunger",
        "famine", "humanitarian", "aid", "poverty", "school", "student", "teacher",
        "education", "housing", "homeless", "welfare", "pension", "wage", "abortion",
        "racism", "discrimination", "vandalism", "violence", "violent", "stabbing", "accused",
        "抗议", "示威", "骚乱", "暴乱", "暴力", "宵禁", "游行", "罢工", "洗劫", "抢劫", "移民",
        "难民", "庇护", "犯罪", "枪击", "警方", "警察", "逮捕", "被捕", "拘留", "监狱", "饥荒",
        "饥饿", "人道", "援助", "贫困", "学生", "学校", "教育", "住房", "民生", "养老", "工资",
        "老龄化", "社会",
    ]),
    ("disaster", &[
        "earthquake", "quake", "flood", "wildfire", "fire", "blaze", "hurricane", "typhoon",
        "cyclone", "storm", "tornado", "crash", "explosion", "blast", "collapse", "derail",
        "landslide", "tsunami", "volcano", "eruption", "drought", "heatwave", "accident",
        "wreck", "rescue", "evacuate", "evacuation",
        "地震", "洪水", "洪灾", "山火", "火灾", "大火", "起火", "台风", "飓风", "风暴", "暴雨",
        "坠毁", "空难", "爆炸", "坍塌", "倒塌", "事故", "海啸", "火山", "干旱", "热浪", "救援",
        "撤离", "遇难", "失事", "迫降",
    ]),
    ("economy", &[
        "economy", "economic", "gdp", "inflation", "rate", "interest", "fed", "ecb", "boj",
        "pboc", "bank", "bond", "yield", "treasury", "stock", "equity", "market", "tariff",
        "oil", "crude", "diesel", "fuel", "opec", "gold", "currency", "euro", "dollar", "yuan",
        "yen", "recession", "job", "unemployment", "payroll", "debt", "deficit", "budget",
        "trade", "export", "import", "cpi", "investor", "selloff", "tax", "surplus",
        "economist",
        "经济", "通胀", "利率", "央行", "美联储", "加息", "降息", "国债", "收益率", "股市", "股票",
        "美股", "港股", "市场", "关税", "原油", "石油", "油价", "柴油", "燃油", "黄金", "汇率",
        "人民币", "美元", "衰退", "就业", "失业", "债务", "赤字", "预算", "贸易", "出口", "进口",
        "增速", "投资者", "抛售", "国内生产总值", "房价", "税",
    ]),
    ("business", &[
        "company", "earnings", "revenue", "profit", "ipo", "acquisition", "acquire", "merger",
        "ceo", "layoff", "startup", "funding", "valuation", "investment", "retailer", "retail",
        "brand", "shareholder", "share", "factory", "automaker", "airline", "lawsuit",
        "antitrust",
        "公司", "企业", "财报", "营收", "利润", "收购", "并购", "上市", "裁员", "融资", "估值",
        "股东", "品牌", "零售", "工厂", "车企", "航空", "诉讼", "反垄断",
    ]),
    ("ai", &[
        "ai", "llm", "gpt", "chatgpt", "openai", "anthropic", "claude", "gemini", "deepseek",
        "qwen", "kimi", "minimax", "mistral", "llama", "copilot", "agent", "model", "inference",
        "deepmind", "neural", "chatbot", "agi",
        "人工智能", "大模型", "模型", "智能体", "机器学习", "算力",
    ]),
    ("tech", &[
        "tech", "technology", "software", "chip", "semiconductor", "nvidia", "apple", "google",
        "microsoft", "meta", "amazon", "iphone", "ipad", "mac", "android", "linux", "kernel",
        "app", "smartphone", "robot", "robotics", "tesla", "spacex", "starlink", "satellite",
        "browser", "github", "rust", "python", "javascript", "npm", "cloud", "datacenter", "gpu",
        "cpu", "headset", "firmware",
        "科技", "芯片", "半导体", "软件", "苹果", "谷歌", "微软", "英伟达", "手机", "内核", "机器人",
        "特斯拉", "电动车", "卫星", "星链", "开源", "浏览器", "数据中心", "华为", "小米",
    ]),
    ("cyber", &[
        "vulnerability", "cve", "exploit", "breach", "hack", "hacker", "malware", "ransomware",
        "phishing", "cyber", "cyberattack", "cybersecurity", "ddos", "advisory", "zeroday",
        "spyware", "backdoor",
        "漏洞", "黑客", "勒索", "网络攻击", "数据泄露", "恶意软件", "钓鱼", "网络安全", "后门",
        "间谍软件",
    ]),
    ("science", &[
        "study", "research", "researcher", "scientist", "science", "physics", "math",
        "mathematics", "hypothesis", "theorem", "astronomy", "space", "nasa", "telescope",
        "climate", "emission", "health", "disease", "vaccine", "virus", "outbreak", "ebola",
        "cancer", "drug", "fda", "medical", "hospital", "patient", "gene", "genome", "protein",
        "species", "fossil", "nhs", "evolution",
        "研究", "科学家", "科学", "物理", "数学", "猜想", "天文", "太空", "航天", "气候", "排放",
        "健康", "疾病", "疫苗", "病毒", "疫情", "埃博拉", "癌症", "药物", "医疗", "医院", "患者",
        "基因", "蛋白", "物种", "论文",
    ]),
    ("crypto", &[
        "bitcoin", "btc", "ethereum", "eth", "crypto", "cryptocurrency", "stablecoin", "defi",
        "token", "solana", "coinbase", "binance", "blockchain", "onchain", "nft", "whale",
        "airdrop", "memecoin", "xrp", "usdt", "usdc", "tether", "saylor",
        "比特币", "以太坊", "加密", "稳定币", "代币", "区块链", "币安", "链上", "巨鲸", "空投",
    ]),
    ("culture", &[
        "football", "soccer", "nba", "wnba", "nfl", "mlb", "nhl", "league", "championship",
        "tournament", "olympic", "olympics", "coach", "player", "athlete", "goal", "cricket",
        "tennis", "rugby", "film", "movie", "cinema", "music", "album", "singer", "actor",
        "actress", "celebrity", "museum", "painting", "exhibition", "festival", "novel",
        "theatre", "theater", "concert", "artist", "culture", "cultural", "fashion",
        "足球", "篮球", "体育", "联赛", "冠军", "奥运", "球员", "电影", "音乐", "歌手", "演员",
        "明星", "博物馆", "艺术", "展览", "文化", "小说", "演唱会", "综艺", "游戏",
    ]),
];

/// 按信号词计分选栏目：标题命中记 2 分，正文（摘要）命中记 1 分，同一个词只算一次。
/// 不再按“第一个命中”选，避免“总理宣布宵禁以遏制骚乱”因为先命中 government 一类词而分错。
fn report_section(title: &str, body: &str) -> &'static str {
    let title = title.to_lowercase();
    let body = body.to_lowercase();
    let title_tokens = ascii_tokens(&title);
    let body_tokens = ascii_tokens(&body);
    let mut best = ("general", 0);
    for (section, markers) in SECTION_MARKERS {
        let score = markers
            .iter()
            .map(|marker| {
                if section_marker_hit(&title, &title_tokens, marker) {
                    2
                } else if section_marker_hit(&body, &body_tokens, marker) {
                    1
                } else {
                    0
                }
            })
            .sum::<i32>();
        if score > best.1 {
            best = (section, score);
        }
    }
    best.0
}

fn ascii_tokens(text: &str) -> Vec<&str> {
    text.split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect()
}

fn section_marker_hit(text: &str, tokens: &[&str], marker: &str) -> bool {
    if !marker.is_ascii() {
        return text.contains(marker);
    }
    tokens.iter().any(|token| marker_inflection(token, marker))
}

/// 词尾宽松匹配：riot → riots / rioting / rioters，invade → invaded。
/// 三个字母以内的词（ai、aid、fed）只做整词匹配，避免 ai 命中 aid。
fn marker_inflection(token: &str, marker: &str) -> bool {
    if token == marker {
        return true;
    }
    if marker.len() <= 3 {
        return false;
    }
    let Some(suffix) = token.strip_prefix(marker) else {
        return false;
    };
    matches!(suffix, "s" | "es" | "ed" | "ing" | "er" | "ers")
        || (marker.ends_with('e') && matches!(suffix, "d" | "r" | "rs"))
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

fn normalize_report_hours(hours: i64) -> i64 {
    if hours <= 12 { 12 } else { 24 }
}

pub(crate) fn report_limit(hours: i64) -> usize {
    if normalize_report_hours(hours) == 12 {
        60
    } else {
        90
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

/// 供其他模块测试构造入选条目。
#[cfg(test)]
pub(crate) fn test_item(title: &str, independent_source_count: i32) -> EventBriefItem {
    tests::test_brief_item(title, 70, 70, 70, 70, independent_source_count, "Source")
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn test_brief_item(
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
            report_section: report_section(title, title).to_string(),
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
            source_families: vec![source_title.to_lowercase()],
            watch_ids: Vec::new(),
            watch_names: Vec::new(),
            representative_article_id: Uuid::new_v4(),
            published_at: Utc::now(),
            update_count: 1,
            has_grounded_summary: false,
            feedback: None,
            editorial_profile: None,
            editorial_score: None,
            editorial_tags: Vec::new(),
            history_note: None,
            history_refs: Vec::new(),
            community_note: None,
            takeaway: None,
            below_editorial_threshold: false,
            awaiting_assessment: false,
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
    fn comprehensive_reports_use_half_day_or_full_day_windows() {
        assert_eq!(normalize_report_hours(1), 12);
        assert_eq!(normalize_report_hours(12), 12);
        assert_eq!(normalize_report_hours(18), 24);
        assert_eq!(normalize_report_hours(72), 24);
        assert_eq!(report_limit(12), 60);
        assert_eq!(report_limit(24), 90);
    }

    #[test]
    fn report_sections_have_stable_user_facing_labels() {
        let section = |title: &str| report_section(title, "");
        assert_eq!(section("Election date confirmed"), "politics");
        assert_eq!(section("Major vulnerability disclosed"), "cyber");
        assert_eq!(section("Central bank changes rates"), "economy");
        assert_eq!(section("Something unusual happened today"), "general");
        assert_eq!(section("OpenAI ships a new reasoning model"), "ai");
        assert_eq!(section("Nvidia unveils new GPU for gaming laptops"), "tech");
        assert_eq!(section("Houthi drone attack hits Saudi airport"), "conflict");
        assert_eq!(section("Strong earthquake strikes off Japan coast, tsunami warning issued"), "disaster");
        assert_eq!(section("Bitcoin slips 2% as ETF outflows continue"), "crypto");
        assert_eq!(section("New Ebola outbreak confirmed in Congo"), "science");
        assert_eq!(section("Retailer cuts 3,000 jobs after weak earnings"), "business");
        // 2026-10-19 法国骚乱：总理、宵禁、骚乱同时出现时归社会民生，不再落入“其他”。
        assert_eq!(
            section("French prime minister calls for limited curfews to curb rioting and looting"),
            "society"
        );
        assert_eq!(section("法国暴力骚乱持续蔓延 总理勒科尔尼：必要时宣布紧急状态"), "society");
        assert_eq!(section("France protests escalate with riots, looting and 2,400 arrests"), "society");
        // 正文补充信号：标题看不出栏目时按摘要判断。
        assert_eq!(report_section("What happens now?", "数学家用 AI 证明了一个猜想"), "science");
        // 短词只做整词匹配：aid 不命中 ai。
        assert_eq!(section("WFP announces food aid cuts amid hunger"), "society");
        assert_eq!(section("Arsenal beat Forest as Premier League title race tightens"), "culture");
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
        assert_eq!(report_section("Chair said plans remain unchanged", ""), "general");
        assert_eq!(report_section("New AI model rules announced", ""), "ai");
        assert!(
            impact_signal("Military strikes close a major shipping route", "")
                > impact_signal("Championship final preview", "")
        );
    }

    fn plain_candidate(title: &str) -> Candidate {
        Candidate {
            event_id: Uuid::new_v4(),
            title: title.to_string(),
            representative_article_id: Uuid::new_v4(),
            source_titles: vec!["Wire".to_string(), "Paper".to_string()],
            source_families: vec!["wire.example".to_string(), "paper.example".to_string()],
            source_count: 2,
            independent_source_count: 2,
            article_count: 2,
            update_count: 1,
            average_source_quality: 70,
            published_at: Utc::now(),
            summary: None,
            summary_confidence: None,
            grounding_score: None,
            excerpt: "excerpt".to_string(),
            revision_type: "ADD".to_string(),
            feedback: None,
            content_hash: Uuid::new_v4().to_string(),
            url_hash: Uuid::new_v4().to_string(),
            assessment: None,
        }
    }

    #[test]
    fn assessment_replaces_keyword_impact_and_reason() {
        let mut candidate = plain_candidate("Championship final preview");
        let keyword_only = score_candidate(candidate.clone(), &[], &[], &EditorialConfig::default());
        candidate.assessment = Some(Assessment {
            profile: "world-news".to_string(),
            score: 8.6,
            reason: "影响两国边境数十万居民".to_string(),
            summary: "双方签署停火协议".to_string(),
            history_note: Some("本站 10 月 1 日记录过谈判开始".to_string()),
            ..Assessment::default()
        });
        let assessed = score_candidate(candidate, &[], &[], &EditorialConfig::default());
        assert_eq!(assessed.impact_score, 86);
        assert!(assessed.total_score > keyword_only.total_score);
        assert_eq!(assessed.why_it_matters, "影响两国边境数十万居民");
        assert_eq!(assessed.what_happened, "双方签署停火协议", "editorial summary replaces a raw excerpt");
        assert_eq!(assessed.editorial_score, Some(8.6));
        assert!(passes_evidence_gate(&assessed));
    }

    #[test]
    fn assessments_below_the_profile_threshold_leave_the_brief() {
        let mut candidate = plain_candidate("Military strikes close a major shipping route");
        candidate.assessment = Some(Assessment {
            profile: "world-news".to_string(),
            score: 3.0,
            reason: "例行表态".to_string(),
            summary: "官员重复既有立场".to_string(),
            ..Assessment::default()
        });
        let item = score_candidate(candidate, &[], &[], &EditorialConfig::default());
        assert!(item.below_editorial_threshold);
        assert!(!passes_evidence_gate(&item));
    }

    #[test]
    fn assessment_section_and_headline_override_keywords() {
        let mut candidate = plain_candidate("🖼 Go will reject certificate chains containing eight distrusted SPKIs");
        candidate.assessment = Some(Assessment {
            profile: "tech-research".to_string(),
            score: 6.0,
            reason: "影响使用 Go 标准库校验 TLS 的服务".to_string(),
            summary: "Go 将拒绝含八个不受信任公钥的证书链".to_string(),
            section: Some("cyber".to_string()),
            headline: Some("Go 将拒绝八个不受信任公钥签发的证书链".to_string()),
            ..Assessment::default()
        });
        let item = score_candidate(candidate.clone(), &[], &[], &EditorialConfig::default());
        assert_eq!(item.report_section, "cyber");
        assert_eq!(item.title, "Go 将拒绝八个不受信任公钥签发的证书链");

        candidate.title = "Go will reject certificate chains containing eight distrusted SPKIs".to_string();
        let item = score_candidate(candidate, &[], &[], &EditorialConfig::default());
        assert_eq!(item.title, "Go will reject certificate chains containing eight distrusted SPKIs", "clean titles stay original");
    }

    #[test]
    fn noisy_titles_are_social_posts_not_headlines() {
        assert!(title_is_noisy("🖼 Anthropic宣布拟将公司5%股份投入公共利益基金"));
        assert!(title_is_noisy("↩️ 多省份发生用户访问请求异常跳转至博彩网站事件"));
        assert!(title_is_noisy("JUST IN: China announces new export restrictions on drones"));
        assert!(title_is_noisy("[AINews] not much happened today"));
        assert!(title_is_noisy(&"美国特使威特科夫正前往奥斯陆".repeat(5)));
        assert!(!title_is_noisy("‘Open your doors for AI’: German chancellor urges businesses"));
        assert!(!title_is_noisy("US bans DeepSeek from civilian agency IT systems"));
        assert!(!title_is_noisy("日本 MMX 探测器升空，将赴火卫一采样"));
    }

    #[test]
    fn unassessed_single_source_events_wait_for_review_when_editorial_is_on() {
        let enabled = EditorialConfig { enabled: true, ..EditorialConfig::default() };
        let mut candidate = plain_candidate("Military strikes close a major shipping route");
        candidate.independent_source_count = 1;
        let waiting = score_candidate(candidate.clone(), &[], &[], &enabled);
        assert!(waiting.awaiting_assessment);
        assert!(!passes_evidence_gate(&waiting));
        let fallback = score_candidate(candidate.clone(), &[], &[], &EditorialConfig::default());
        assert!(!fallback.awaiting_assessment, "keyword scoring still works without the editorial layer");
        let mut multi = candidate;
        multi.independent_source_count = 3;
        assert!(!score_candidate(multi, &[], &[], &enabled).awaiting_assessment);
        let mut forum = plain_candidate("出山姆会员副卡");
        forum.source_families = vec!["rsshub:v2ex/topics".to_string(), "nodeseek.com".to_string()];
        assert!(score_candidate(forum, &[], &[], &enabled).awaiting_assessment, "cross-posted forum threads are not independent");
    }

    #[test]
    fn section_shares_cap_a_dominant_section() {
        let items = (0..6)
            .map(|index| {
                test_brief_item(&format!("Military unit{index} alpha{index} bravo{index}"), 90 - index, 80, 80, 80, 2, &format!("Source {index}"))
            })
            .chain((0..3).map(|index| {
                let titles = ["New AI chip rules announced", "Software vendor ships database", "Technology standard body votes"];
                test_brief_item(titles[index], 70 - index as i32, 70, 70, 70, 2, &format!("Tech {index}"))
            }))
            .collect::<Vec<_>>();
        let shares = HashMap::from([("conflict".to_string(), 0.4)]);
        let selected = select_diverse(items, 6, &shares);
        let conflict = selected.iter().filter(|item| item.report_section == "conflict").count();
        assert_eq!(conflict, 3, "ceil(6 * 0.4) conflict items");
        assert_eq!(selected.len(), 6);
    }

    #[test]
    fn brief_items_expose_persisted_feedback_state() {
        let mut candidate = Candidate {
            event_id: Uuid::nil(),
            title: "A verified event changed".to_string(),
            representative_article_id: Uuid::nil(),
            source_titles: vec!["Source".to_string()],
            source_families: vec!["source.example".to_string()],
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
            assessment: None,
        };
        let item = score_candidate(candidate.clone(), &[], &[], &EditorialConfig::default());

        assert_eq!(item.feedback.as_deref(), Some("known"));
        assert!(item.has_grounded_summary);
        candidate.grounding_score = None;
        assert!(!score_candidate(candidate.clone(), &[], &[], &EditorialConfig::default()).has_grounded_summary);
        candidate.grounding_score = Some(90);
        candidate.summary = Some("  ".into());
        assert!(!score_candidate(candidate, &[], &[], &EditorialConfig::default()).has_grounded_summary);
    }

    #[test]
    fn publisher_penalty_uses_identity_instead_of_feed_display_names() {
        let mut selected =
            test_brief_item("Election results announced", 80, 80, 80, 80, 1, "BBC World");
        selected.source_families = vec!["bbc.co.uk".into()];
        let mut candidate =
            test_brief_item("New AI model released", 75, 75, 75, 75, 1, "BBC Technology");
        candidate.source_families = selected.source_families.clone();
        let repeated = selection_score(&candidate, &[selected.clone()], 12);
        candidate.source_titles = vec!["Renamed feed".into()];
        assert_eq!(
            selection_score(&candidate, &[selected.clone()], 12),
            repeated
        );
        candidate.source_families = vec!["another.example".into()];
        assert!(selection_score(&candidate, &[selected], 12) > repeated);
        assert!(
            serde_json::to_value(candidate)
                .unwrap()
                .get("source_families")
                .is_none()
        );
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
        assert_eq!(twelve_hours, 71);
        assert_eq!(one_day, 50);
        assert!(freshness_score(12.0, 2, "UPDATE") > twelve_hours);
    }

    /// 增量实现必须与逐轮全量比较的原实现选出同样的条目和顺序。
    #[test]
    fn incremental_selection_matches_full_rescan() {
        fn reference(
            mut items: Vec<EventBriefItem>,
            limit: usize,
            shares: &HashMap<String, f32>,
        ) -> Vec<Uuid> {
            items.retain(passes_evidence_gate);
            let mut selected = Vec::<EventBriefItem>::new();
            let mut counts = HashMap::<String, usize>::new();
            while !items.is_empty() && selected.len() < limit {
                items.retain(|item| {
                    let full = section_cap(&item.report_section, limit, shares)
                        .is_some_and(|cap| counts.get(&item.report_section).copied().unwrap_or(0) >= cap);
                    !full
                        && !selected.iter().any(|chosen| {
                            chosen.content_hash == item.content_hash
                                || chosen.url_hash == item.url_hash
                                || titles_same_brief_event(&chosen.title, &item.title)
                        })
                });
                let Some(best) = items
                    .iter()
                    .enumerate()
                    .max_by(|(_, l), (_, r)| {
                        selection_score(l, &selected, limit)
                            .total_cmp(&selection_score(r, &selected, limit))
                            .then_with(|| l.total_score.cmp(&r.total_score))
                            .then_with(|| l.published_at.cmp(&r.published_at))
                    })
                    .map(|(index, _)| index)
                else {
                    break;
                };
                let item = items.swap_remove(best);
                *counts.entry(item.report_section.clone()).or_default() += 1;
                selected.push(item);
            }
            selected.into_iter().map(|item| item.event_id).collect()
        }
        let words = [
            "France", "curfew", "riots", "Paris", "Ukraine", "Zelenskyy", "Trump", "sanctions",
            "Russia", "Bitcoin", "ETF", "BlackRock", "Apple", "launch", "October", "Sudan",
            "drone", "attack", "Kenya", "Ebola", "Alberta", "referendum", "Canada", "AI", "chip",
            "Nvidia", "election", "Brazil", "court", "Meta",
        ];
        let mut seed = 42_u64;
        let mut next = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as usize
        };
        let items = (0..400)
            .map(|index| {
                let title = (0..5).map(|_| words[next() % words.len()]).collect::<Vec<_>>().join(" ");
                let mut item = test_brief_item(&title, 55 + (next() % 40) as i32, 70, 70, 70, 1 + (next() % 4) as i32, &format!("Source {}", next() % 12));
                item.published_at = Utc::now() - chrono::Duration::minutes(index);
                item
            })
            .collect::<Vec<_>>();
        let shares = HashMap::from([("conflict".to_string(), 0.2), ("politics".to_string(), 0.25)]);
        let fast = select_diverse(items.clone(), 60, &shares)
            .into_iter()
            .map(|item| item.event_id)
            .collect::<Vec<_>>();
        assert!(fast.len() > 10);
        assert_eq!(fast, reference(items, 60, &shares));
    }

    #[test]
    fn day_old_major_events_outrank_fresh_minor_ones_in_reports() {
        let assessed = |score: f32| Assessment {
            profile: "world-news".to_string(),
            score,
            reason: "r".to_string(),
            summary: "s".to_string(),
            ..Assessment::default()
        };
        let mut major = plain_candidate("Trump suspends some Russia sanctions by executive order");
        major.published_at = Utc::now() - chrono::Duration::hours(18);
        // 分量取自现网两条：多源事件文章多（新颖度被重复扣分）、没有更新；单源快讯信源质量高。
        major.independent_source_count = 13;
        major.article_count = 6;
        major.update_count = 0;
        major.average_source_quality = 80;
        major.assessment = Some(assessed(7.8));
        let mut minor = plain_candidate("US pending home sales fall");
        minor.published_at = Utc::now() - chrono::Duration::minutes(30);
        minor.independent_source_count = 1;
        minor.article_count = 1;
        minor.update_count = 1;
        minor.average_source_quality = 84;
        minor.assessment = Some(assessed(5.5));
        let major = score_candidate(major, &[], &[], &EditorialConfig::default());
        let minor = score_candidate(minor, &[], &[], &EditorialConfig::default());
        assert!(major.total_score > minor.total_score, "{} vs {}", major.total_score, minor.total_score);
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
            "rss.nodeseek.com".to_string(),
            "v2ex.com".to_string()
        ]));
        assert!(!sources_are_community_only(&[
            "bbc.com".to_string(),
            "nodeseek.com".to_string()
        ]));
    }

    #[test]
    fn community_identity_uses_host_boundaries_and_explicit_routes() {
        for identity in [
            "LINUX.DO",
            "www.v2ex.com",
            "rsshub:v2ex/topics/latest",
            "rsshub:linuxdo/latest",
        ] {
            assert!(sources_are_community_only(&[identity.to_string()]));
        }
        for identity in [
            "notv2ex.com",
            "v2ex.com.example.org",
            "rsshub:v2ex-news/latest",
            "Example NodeSeek News",
        ] {
            assert!(!sources_are_community_only(&[identity.to_string()]));
        }
        assert!(!sources_are_community_only(&[]));
    }

    #[test]
    fn renaming_a_feed_does_not_change_community_gating() {
        let mut candidate = Candidate {
            event_id: Uuid::new_v4(),
            title: "AI chip export rules tighten again".to_string(),
            representative_article_id: Uuid::new_v4(),
            source_titles: vec!["World Wire".to_string()],
            source_families: vec!["wire.example".to_string()],
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
            assessment: None,
        };
        candidate.source_families = vec!["rss.nodeseek.com".into()];
        let before = score_candidate(candidate.clone(), &[], &[], &EditorialConfig::default());
        candidate.source_titles = vec!["Renamed source".into()];
        let after = score_candidate(candidate.clone(), &[], &[], &EditorialConfig::default());
        assert!(before.community_only && after.community_only);
        assert_eq!(passes_evidence_gate(&before), passes_evidence_gate(&after));
        candidate.source_families = vec!["news.example.org".into()];
        candidate.source_titles = vec!["NodeSeek review".into()];
        assert!(!score_candidate(candidate, &[], &[], &EditorialConfig::default()).community_only);
    }

    #[test]
    fn preference_feedback_changes_related_events_not_workflow_state() {
        let candidate = Candidate {
            event_id: Uuid::new_v4(),
            title: "AI chip export rules tighten again".to_string(),
            representative_article_id: Uuid::new_v4(),
            source_titles: vec!["World Wire".to_string()],
            source_families: vec!["wire.example".to_string()],
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
            assessment: None,
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

        let selected = select_diverse(vec![same_lane, broader.clone(), first.clone()], 2, &HashMap::new());
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

        let selected = select_diverse(vec![duplicate, distinct.clone(), first.clone()], 2, &HashMap::new());
        assert_eq!(selected[0].event_id, first.event_id);
        assert_eq!(selected[1].event_id, distinct.event_id);
    }
}
