//! 面向交易 Agent 的只读情报接口。所有查询必须给出 `as_of`，只返回当时已入库的材料，
//! 防止回测读到未来信息。`known_at` 取文章首次入库时间 `articles.created_at`。
//!
//! 已知局限：事件分组使用当前聚类结果（之后的重聚类可能把当时尚未合并的报道归为一组）；
//! 佐证来源数只统计 `as_of` 之前入库的文章，但分组本身不是时间点快照。

use axum::{
    Json, Router,
    extract::{Query, State},
    http::HeaderMap,
    routing::get,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    auth,
    error::{AppError, AppResult},
    feeds::excerpt,
    state::AppState,
};

const MAX_LOOKBACK_HOURS: i64 = 24 * 14;
const DEFAULT_LOOKBACK_HOURS: i64 = 24;
const MAX_LIMIT: i64 = 50;
const DEFAULT_LIMIT: i64 = 20;
const AS_OF_CLOCK_SKEW_SECONDS: i64 = 120;
const GLOBAL_MIN_QUALITY: i32 = 60;
const EXCERPT_CHARS: usize = 600;
const BODY_MIN_MENTIONS: i64 = 3;

/// 资产代码到匹配正则（PostgreSQL ARE，`\m` / `\M` 为词边界；中文不加边界）。
const ASSET_PATTERNS: &[(&str, &str)] = &[
    ("BTC", r"\mbitcoin\M|\mbtc\M|比特币"),
    ("ETH", r"\methereum\M|\meth\M|\mether\M|以太坊"),
    ("SOL", r"\msolana\M|索拉纳"),
    ("XRP", r"\mxrp\M|\mripple\M|瑞波"),
    ("BNB", r"\mbnb\M|\mbinance coin\M"),
    ("DOGE", r"\mdogecoin\M|\mdoge\M|狗狗币"),
    (
        "CRYPTO",
        r"\mcrypto\M|\mcryptocurrenc|\mstablecoin|\mdefi\M|\mbinance\M|\mcoinbase\M|加密货币|数字货币|稳定币|币安",
    ),
];

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v2/intel/news", get(asset_news))
        .route("/api/v2/intel/global", get(global_news))
}

#[derive(Debug, Deserialize)]
struct IntelQuery {
    as_of: Option<String>,
    asset: Option<String>,
    lookback_hours: Option<i64>,
    limit: Option<i64>,
}

#[derive(Debug, Serialize)]
struct IntelResponse {
    as_of: DateTime<Utc>,
    window_start: DateTime<Utc>,
    asset: Option<String>,
    known_at_semantics: &'static str,
    items: Vec<IntelItem>,
}

#[derive(Debug, Serialize)]
struct IntelItem {
    article_id: Uuid,
    story_id: Option<Uuid>,
    title: String,
    url: String,
    source: String,
    source_type: Option<String>,
    language: Option<String>,
    quality_score: i32,
    published_at: Option<DateTime<Utc>>,
    known_at: DateTime<Utc>,
    corroborating_sources: i64,
    excerpt: String,
}

struct Window {
    as_of: DateTime<Utc>,
    start: DateTime<Utc>,
    limit: i64,
}

async fn asset_news(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<IntelQuery>,
) -> AppResult<Json<IntelResponse>> {
    auth::require_intel_access(&state.pool, &state.config, &headers).await?;
    let window = parse_window(&query, Utc::now())?;
    let (asset, pattern) = asset_pattern(query.asset.as_deref().unwrap_or_default())?;
    let items = load_items(&state, &window, Some(pattern), 0).await?;
    Ok(Json(response(&window, Some(asset.to_string()), items)))
}

async fn global_news(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<IntelQuery>,
) -> AppResult<Json<IntelResponse>> {
    auth::require_intel_access(&state.pool, &state.config, &headers).await?;
    let window = parse_window(&query, Utc::now())?;
    let items = load_items(&state, &window, None, GLOBAL_MIN_QUALITY).await?;
    Ok(Json(response(&window, None, items)))
}

fn response(window: &Window, asset: Option<String>, items: Vec<IntelItem>) -> IntelResponse {
    IntelResponse {
        as_of: window.as_of,
        window_start: window.start,
        asset,
        known_at_semantics: "first_ingested_at",
        items,
    }
}

fn parse_window(query: &IntelQuery, now: DateTime<Utc>) -> AppResult<Window> {
    let raw = query
        .as_of
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::BadRequest("as_of 必填，格式为 RFC 3339 时间".to_string()))?;
    let as_of = DateTime::parse_from_rfc3339(raw)
        .map_err(|_| AppError::BadRequest("as_of 必须是 RFC 3339 时间，例如 2026-10-12T08:00:00Z".to_string()))?
        .with_timezone(&Utc);
    if as_of > now + Duration::seconds(AS_OF_CLOCK_SKEW_SECONDS) {
        return Err(AppError::BadRequest("as_of 不能晚于当前时间".to_string()));
    }
    let lookback = query.lookback_hours.unwrap_or(DEFAULT_LOOKBACK_HOURS);
    if !(1..=MAX_LOOKBACK_HOURS).contains(&lookback) {
        return Err(AppError::BadRequest(format!(
            "lookback_hours 必须在 1–{MAX_LOOKBACK_HOURS} 之间"
        )));
    }
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT);
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(AppError::BadRequest(format!("limit 必须在 1–{MAX_LIMIT} 之间")));
    }
    Ok(Window {
        as_of,
        start: as_of - Duration::hours(lookback),
        limit,
    })
}

/// 接受 `BTC`、`btc`、`BTCUSDT`、`BTC-USD` 等写法。
fn asset_pattern(raw: &str) -> AppResult<(&'static str, &'static str)> {
    let normalized = raw.trim().to_ascii_uppercase().replace(['-', '/', '_'], "");
    let base = ["USDT", "USDC", "USD"]
        .iter()
        .find_map(|quote| {
            normalized
                .strip_suffix(quote)
                .filter(|base| !base.is_empty())
        })
        .unwrap_or(&normalized);
    ASSET_PATTERNS
        .iter()
        .find(|(asset, _)| *asset == base)
        .copied()
        .ok_or_else(|| {
            let supported = ASSET_PATTERNS
                .iter()
                .map(|(asset, _)| *asset)
                .collect::<Vec<_>>()
                .join(", ");
            AppError::BadRequest(format!("asset 不支持，可用：{supported}"))
        })
}

async fn load_items(
    state: &AppState,
    window: &Window,
    pattern: Option<&str>,
    min_quality: i32,
) -> AppResult<Vec<IntelItem>> {
    // 标题命中资产，或正文前 4000 字至少命中 BODY_MIN_MENTIONS 次，过滤顺带一提的文章。
    // 每个事件只保留质量最高、最早入库的一篇；佐证来源数只计 as_of 之前入库的不同订阅源。
    let rows = sqlx::query(
        "WITH win AS (
           SELECT a.id, a.story_id, a.title, a.url, a.language, a.quality_score,
                  a.published_at, a.created_at AS known_at,
                  COALESCE(NULLIF(a.extracted_content, ''), a.content) AS body,
                  f.title AS source, f.source_type,
                  ($3::text IS NOT NULL AND a.title ~* $3) AS title_match
           FROM articles a
           JOIN feeds f ON f.id = a.feed_id
           WHERE a.created_at <= $1 AND a.created_at > $2
             AND a.quality_score >= $4
             AND ($3::text IS NULL
                  OR a.title ~* $3
                  OR (SELECT count(*) FROM regexp_matches(
                        left(COALESCE(NULLIF(a.extracted_content, ''), a.content), 4000), $3, 'gi'
                      )) >= $6)
         ),
         picked AS (
           SELECT DISTINCT ON (COALESCE(story_id, id)) *
           FROM win
           ORDER BY COALESCE(story_id, id), quality_score DESC, known_at ASC
         ),
         counts AS (
           SELECT a.story_id, count(DISTINCT a.feed_id) AS sources
           FROM articles a
           WHERE a.story_id IN (SELECT story_id FROM picked WHERE story_id IS NOT NULL)
             AND a.created_at <= $1
           GROUP BY a.story_id
         )
         SELECT picked.*, COALESCE(counts.sources, 1) AS corroborating_sources
         FROM picked
         LEFT JOIN counts USING (story_id)
         ORDER BY CASE WHEN $3::text IS NULL THEN COALESCE(counts.sources, 1) ELSE 0 END DESC,
                  title_match DESC,
                  known_at DESC,
                  quality_score DESC
         LIMIT $5",
    )
    .bind(window.as_of)
    .bind(window.start)
    .bind(pattern)
    .bind(min_quality)
    .bind(window.limit)
    .bind(BODY_MIN_MENTIONS)
    .fetch_all(&state.pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| IntelItem {
            article_id: row.get("id"),
            story_id: row.get("story_id"),
            title: row.get("title"),
            url: row.get("url"),
            source: row.get("source"),
            source_type: row.get("source_type"),
            language: row.get("language"),
            quality_score: row.get("quality_score"),
            published_at: row.get("published_at"),
            known_at: row.get("known_at"),
            corroborating_sources: row.get("corroborating_sources"),
            excerpt: excerpt(&row.get::<String, _>("body"), EXCERPT_CHARS),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(as_of: Option<&str>, lookback: Option<i64>, limit: Option<i64>) -> IntelQuery {
        IntelQuery {
            as_of: as_of.map(str::to_string),
            asset: None,
            lookback_hours: lookback,
            limit,
        }
    }

    #[test]
    fn requires_valid_past_as_of() {
        let now = DateTime::parse_from_rfc3339("2026-10-12T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(parse_window(&query(None, None, None), now).is_err());
        assert!(parse_window(&query(Some("2026-10-12"), None, None), now).is_err());
        assert!(parse_window(&query(Some("2026-10-12T13:00:00Z"), None, None), now).is_err());

        let window = parse_window(&query(Some("2026-10-12T08:00:00+08:00"), None, None), now).unwrap();
        assert_eq!(window.as_of.to_rfc3339(), "2026-10-12T00:00:00+00:00");
        assert_eq!(window.start.to_rfc3339(), "2026-10-11T00:00:00+00:00");
        assert_eq!(window.limit, DEFAULT_LIMIT);
    }

    #[test]
    fn bounds_lookback_and_limit() {
        let now = Utc::now();
        let as_of = (now - Duration::hours(1)).to_rfc3339();
        assert!(parse_window(&query(Some(&as_of), Some(0), None), now).is_err());
        assert!(parse_window(&query(Some(&as_of), Some(MAX_LOOKBACK_HOURS + 1), None), now).is_err());
        assert!(parse_window(&query(Some(&as_of), None, Some(MAX_LIMIT + 1)), now).is_err());
        assert!(parse_window(&query(Some(&as_of), Some(MAX_LOOKBACK_HOURS), Some(MAX_LIMIT)), now).is_ok());
    }

    #[test]
    fn normalizes_asset_symbols() {
        for raw in ["BTC", "btc", "BTCUSDT", "BTC-USD", "btc/usdc"] {
            assert_eq!(asset_pattern(raw).unwrap().0, "BTC", "{raw}");
        }
        assert_eq!(asset_pattern("ethusdt").unwrap().0, "ETH");
        assert_eq!(asset_pattern("crypto").unwrap().0, "CRYPTO");
        assert!(asset_pattern("").is_err());
        assert!(asset_pattern("USDT").is_err());
        assert!(asset_pattern("AAPL").is_err());
    }
}
