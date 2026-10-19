//! 编辑层：LLM 评审（Profile 路由与评分、此前进展、社区观点）、信源画像和日报。
//! 设计与配置见 docs/EDITORIAL_LAYER.md。

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use tokio::time;

use crate::{
    auth,
    error::{AppError, AppResult},
    state::AppState,
};

pub mod assess;
pub mod community;
pub mod edition;
pub mod profiles;
pub mod sources;
pub mod text;

const EDITORIAL_LOCK_KEY: i64 = 0x5253_4544_4954_0101;
const SOURCE_STATS_INTERVAL: time::Duration = time::Duration::from_secs(6 * 3_600);

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v2/editorial/status", get(status))
        .route(
            "/api/v2/brief/editions",
            get(list_editions).post(publish_edition),
        )
        .route("/api/v2/brief/editions/{date}", get(get_edition))
        .route("/feeds/brief.xml", get(atom_feed))
}

/// 评审、信源画像和日报共用一个后台循环，用 advisory lock 保证多实例时只有一个在跑。
/// 日报和信源画像不依赖评审开关：没有评审结果时日报照常按规则分生成。
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        loop {
            let mut connection = match state.pool.acquire().await {
                Ok(connection) => connection,
                Err(error) => {
                    tracing::warn!(%error, "editorial leader connection failed");
                    time::sleep(time::Duration::from_secs(10)).await;
                    continue;
                }
            };
            let acquired = sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock($1)")
                .bind(EDITORIAL_LOCK_KEY)
                .fetch_one(&mut *connection)
                .await
                .unwrap_or(false);
            if !acquired {
                time::sleep(time::Duration::from_secs(30)).await;
                continue;
            }
            tracing::info!(
                enabled = state.config.editorial.enabled,
                "editorial leader lock acquired"
            );
            let mut last_source_refresh: Option<time::Instant> = None;
            loop {
                if sqlx::query("SELECT 1")
                    .fetch_one(&mut *connection)
                    .await
                    .is_err()
                {
                    tracing::warn!("editorial leader connection lost");
                    break;
                }
                // 先合并同事件簇，评审名额不浪费在即将并掉的碎片上。
                match crate::semantic::consolidate_stories(&state).await {
                    Ok(0) => {}
                    Ok(merged) => tracing::info!(merged, "story consolidation finished"),
                    Err(error) => tracing::warn!(%error, "story consolidation failed"),
                }
                if state.config.editorial.enabled
                    && let Err(error) = assess::run_turn(&state).await
                {
                    tracing::warn!(%error, "editorial assessment turn failed");
                }
                match crate::semantic::retry_failed_links(&state).await {
                    Ok(0) => {}
                    Ok(retried) => tracing::info!(retried, "semantic link retry finished"),
                    Err(error) => tracing::warn!(%error, "semantic link retry failed"),
                }
                if last_source_refresh.is_none_or(|at| at.elapsed() >= SOURCE_STATS_INTERVAL) {
                    match sources::refresh(&state).await {
                        Ok(feeds) => tracing::info!(feeds, "feed quality stats refreshed"),
                        Err(error) => tracing::warn!(%error, "feed quality stats failed"),
                    }
                    last_source_refresh = Some(time::Instant::now());
                }
                if let Err(error) = edition::maybe_publish(&state).await {
                    tracing::warn!(%error, "brief edition failed");
                }
                time::sleep(state.config.editorial.interval).await;
            }
        }
    });
}

#[derive(Debug, Serialize)]
struct StatusResponse {
    enabled: bool,
    prompt_version: &'static str,
    daily_event_limit: i64,
    assessed_last_24h: i64,
    failed_last_24h: i64,
    done_total: i64,
    last_assessed_at: Option<chrono::DateTime<Utc>>,
    last_error: Option<String>,
    profiles: Vec<ProfileStatus>,
}

#[derive(Debug, Serialize)]
struct ProfileStatus {
    id: &'static str,
    name: &'static str,
    threshold: f32,
}

async fn status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<StatusResponse>> {
    auth::require_session(&state.pool, &headers).await?;
    let row = sqlx::query(
        "SELECT
           COUNT(*) FILTER (WHERE assessed_at >= now() - interval '24 hours') AS assessed,
           COUNT(*) FILTER (WHERE assessed_at >= now() - interval '24 hours' AND error IS NOT NULL) AS failed,
           COUNT(*) FILTER (WHERE status = 'done') AS done_total,
           MAX(assessed_at) AS last_assessed_at,
           (SELECT error FROM event_assessments WHERE error IS NOT NULL
            ORDER BY assessed_at DESC LIMIT 1) AS last_error
         FROM event_assessments",
    )
    .fetch_one(&state.pool)
    .await?;
    let editorial = &state.config.editorial;
    Ok(Json(StatusResponse {
        enabled: editorial.enabled,
        prompt_version: assess::PROMPT_VERSION,
        daily_event_limit: editorial.daily_event_limit,
        assessed_last_24h: row.get("assessed"),
        failed_last_24h: row.get("failed"),
        done_total: row.get("done_total"),
        last_assessed_at: row.get("last_assessed_at"),
        last_error: row.get("last_error"),
        profiles: profiles::PROFILES
            .iter()
            .map(|profile| ProfileStatus {
                id: profile.id,
                name: profile.name,
                threshold: editorial.threshold(profile.id),
            })
            .collect(),
    }))
}

#[derive(Debug, Serialize)]
struct EditionSummary {
    edition_date: NaiveDate,
    generated_at: chrono::DateTime<Utc>,
    item_count: i32,
}

#[derive(Debug, Serialize)]
struct EditionList {
    /// 设置了 `BRIEF_FEED_TOKEN` 时为订阅地址（不含 token），否则为空。
    feed_path: Option<&'static str>,
    editions: Vec<EditionSummary>,
}

async fn list_editions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<EditionList>> {
    auth::require_session(&state.pool, &headers).await?;
    let editions = edition::latest(&state, 60).await?;
    Ok(Json(EditionList {
        feed_path: state
            .config
            .editorial
            .brief_feed_token
            .as_ref()
            .map(|_| "/feeds/brief.xml"),
        editions: editions
            .into_iter()
            .map(|edition| EditionSummary {
                edition_date: edition.edition_date,
                generated_at: edition.generated_at,
                item_count: edition.item_count,
            })
            .collect(),
    }))
}

/// 手动生成（或重新生成）今天的日报，不推送。
async fn publish_edition(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<(StatusCode, Json<edition::Edition>)> {
    auth::require_session(&state.pool, &headers).await?;
    let date = Utc::now()
        .with_timezone(&edition::local_offset(&state.config))
        .date_naive();
    Ok((
        StatusCode::CREATED,
        Json(edition::publish(&state, date).await?),
    ))
}

#[derive(Debug, Deserialize)]
struct EditionQuery {
    format: Option<String>,
}

async fn get_edition(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(date): Path<String>,
    Query(query): Query<EditionQuery>,
) -> AppResult<Response> {
    auth::require_session(&state.pool, &headers).await?;
    let date = NaiveDate::parse_from_str(date.trim_end_matches(".md"), "%Y-%m-%d")
        .map_err(|_| AppError::BadRequest("日期格式应为 YYYY-MM-DD".to_string()))?;
    let edition = edition::by_date(&state, date)
        .await?
        .ok_or(AppError::NotFound)?;
    if query.format.as_deref() == Some("markdown") {
        return Ok((
            [(header::CONTENT_TYPE, "text/markdown; charset=utf-8")],
            edition.markdown,
        )
            .into_response());
    }
    Ok(Json(edition).into_response())
}

#[derive(Debug, Deserialize)]
struct FeedQuery {
    token: Option<String>,
}

/// Atom 订阅。阅读器无法带会话 Cookie，所以用独立的只读 token；未配置时不存在。
async fn atom_feed(
    State(state): State<AppState>,
    Query(query): Query<FeedQuery>,
) -> AppResult<Response> {
    let Some(expected) = &state.config.editorial.brief_feed_token else {
        return Err(AppError::NotFound);
    };
    let token = query.token.unwrap_or_default();
    if !auth::constant_time_eq(expected, token.trim()) {
        return Err(AppError::Unauthorized);
    }
    let editions = edition::latest(&state, edition::FEED_EDITIONS).await?;
    let public_url = state.config.app_public_url.trim_end_matches('/');
    let body = edition::render_atom(
        &editions,
        public_url,
        &format!("{public_url}/feeds/brief.xml"),
    );
    Ok((
        [(header::CONTENT_TYPE, "application/atom+xml; charset=utf-8")],
        body,
    )
        .into_response())
}
