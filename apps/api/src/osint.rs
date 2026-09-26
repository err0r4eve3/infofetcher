use std::{collections::HashSet, time::Duration};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use reqwest::Url;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    auth,
    error::{AppError, AppResult},
    feeds::canonicalize_url,
    intelligence::publisher_family,
    llm::GeneratedCitation,
    net,
    state::AppState,
};

const MAX_SOURCES: usize = 60;
const MAX_BYTES: usize = 1_048_576;

mod research;

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(research::router())
        .route("/api/v2/events/{id}/osint", get(dossier))
        .route(
            "/api/v2/events/{id}/evidence/{article_id}/capture",
            post(capture),
        )
        .route("/api/v2/osint/captures/{id}/export", get(export_capture))
}

#[derive(Debug, Serialize)]
struct Source {
    article_id: Uuid,
    title: String,
    source_title: String,
    publisher: String,
    url: String,
    canonical_url: String,
    content_hash: String,
    reported_published_at: Option<DateTime<Utc>>,
    fetched_at: DateTime<Utc>,
    dependency_group: usize,
    latest_capture: Option<Value>,
}

#[derive(Serialize)]
struct Dossier {
    event_id: Uuid,
    title: String,
    sources: Vec<Source>,
    total_sources: i64,
    dependency_groups: usize,
    core_support_groups: usize,
    digest_message: Option<String>,
    digest_completed_at: Option<DateTime<Utc>>,
    citations: Vec<GeneratedCitation>,
    disagreements: Vec<String>,
    gaps: Vec<String>,
}

async fn dossier(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Dossier>> {
    auth::require_session(&state.pool, &headers).await?;
    let event = sqlx::query(
        "SELECT s.representative_title, d.message, d.citations, d.disagreements, d.completed_at
        FROM story_clusters s LEFT JOIN event_digests d ON d.story_id = s.id AND d.status = 'done'
        WHERE s.id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    let rows = sqlx::query("SELECT a.id, a.title, a.url, a.canonical_url, a.content_hash, a.url_hash,
        a.published_at, a.fetched_at, f.title AS source_title, f.site_url, f.feed_url, f.hub_adapter, f.hub_path,
        count(*) OVER () AS total_sources, c.capture
        FROM articles a JOIN feeds f ON f.id = a.feed_id
        LEFT JOIN source_feedback sf ON sf.feed_id = f.id
        LEFT JOIN LATERAL (SELECT jsonb_build_object('id', id, 'captured_at', captured_at,
            'sha256', sha256, 'byte_length', octet_length(body), 'details', details) AS capture
            FROM osint_captures WHERE article_id = a.id ORDER BY captured_at DESC LIMIT 1) c ON true
        WHERE a.story_id = $1 AND NOT COALESCE(sf.muted, false)
        ORDER BY a.quality_score DESC, a.fetched_at DESC, a.id LIMIT $2")
        .bind(id).bind(MAX_SOURCES as i64).fetch_all(&state.pool).await?;
    let total_sources = rows.first().map(|r| r.get("total_sources")).unwrap_or(0);
    let mut sources = rows
        .into_iter()
        .map(|r| {
            let hash: String = r.get("content_hash");
            Source {
                article_id: r.get("id"),
                title: r.get("title"),
                source_title: r.get("source_title"),
                publisher: publisher_family(
                    r.get::<Option<String>, _>("site_url").as_deref(),
                    &r.get::<String, _>("feed_url"),
                    r.get::<Option<String>, _>("hub_adapter").as_deref(),
                    r.get::<Option<String>, _>("hub_path").as_deref(),
                ),
                url: r.get("url"),
                canonical_url: canonicalize_url(&r.get::<String, _>("canonical_url")),
                // Legacy backfill used the URL hash when no content fingerprint existed.
                content_hash: if hash == r.get::<String, _>("url_hash") {
                    String::new()
                } else {
                    hash
                },
                reported_published_at: r.get("published_at"),
                fetched_at: r.get("fetched_at"),
                dependency_group: 0,
                latest_capture: r.get("capture"),
            }
        })
        .collect::<Vec<_>>();
    let dependency_groups = group_sources(&mut sources);
    let citations: Vec<GeneratedCitation> = serde_json::from_value(
        event
            .get::<Option<Value>, _>("citations")
            .unwrap_or(json!([])),
    )
    .map_err(|err| AppError::Anyhow(err.into()))?;
    let citations = citations
        .into_iter()
        .filter(|c| c.verified && sources.iter().any(|s| s.article_id == c.article_id))
        .collect::<Vec<_>>();
    let core_support_groups = core_support(&sources, &citations);
    let mut gaps = vec![
        "不同域名仍可能引用同一原始材料；分组只识别已知依赖，不能证明来源独立。".to_string(),
        "核对原始文件、完整语境及后续更正；材料中的陈述不等于事实已证实。".to_string(),
    ];
    if core_support_groups < 2 {
        gaps.push("核心主张缺少两个不同依赖组的已定位引文；应补充原始记录或独立采访。".to_string());
    }
    if sources.iter().any(|s| s.reported_published_at.is_none()) {
        gaps.push("部分来源未提供发布时间；抓取时间不能替代事件发生时间。".to_string());
    }
    if total_sources > MAX_SOURCES as i64 {
        gaps.push(format!(
            "仅分析质量排序前 {MAX_SOURCES} 篇材料，分组及主张覆盖并非全量。"
        ));
    }
    Ok(Json(Dossier {
        event_id: id,
        title: event.get("representative_title"),
        sources,
        total_sources,
        dependency_groups,
        core_support_groups,
        digest_message: event.get("message"),
        digest_completed_at: event.get("completed_at"),
        citations,
        disagreements: serde_json::from_value(
            event
                .get::<Option<Value>, _>("disagreements")
                .unwrap_or(json!([])),
        )
        .map_err(|err| AppError::Anyhow(err.into()))?,
        gaps,
    }))
}

fn group_sources(sources: &mut [Source]) -> usize {
    // ponytail: O(n²) on at most 60 materials; use indexed union-find if the dossier grows.
    let mut groups = 0;
    for i in 0..sources.len() {
        if sources[i].dependency_group != 0 {
            continue;
        }
        groups += 1;
        sources[i].dependency_group = groups;
        let mut pending = vec![i];
        while let Some(index) = pending.pop() {
            for j in 0..sources.len() {
                if sources[j].dependency_group != 0 {
                    continue;
                }
                let a = &sources[index];
                let b = &sources[j];
                if a.publisher == b.publisher
                    || (!a.canonical_url.is_empty() && a.canonical_url == b.canonical_url)
                    || (!a.content_hash.is_empty() && a.content_hash == b.content_hash)
                {
                    sources[j].dependency_group = groups;
                    pending.push(j);
                }
            }
        }
    }
    groups
}

fn core_support(sources: &[Source], citations: &[GeneratedCitation]) -> usize {
    citations
        .iter()
        .filter(|c| c.verified && c.claim_id == "summary")
        .filter_map(|c| sources.iter().find(|s| s.article_id == c.article_id))
        .map(|s| s.dependency_group)
        .collect::<HashSet<_>>()
        .len()
}

#[derive(Deserialize)]
struct CaptureRequest {
    #[serde(default)]
    include_archive: bool,
}

async fn capture(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((story_id, article_id)): Path<(Uuid, Uuid)>,
    Json(request): Json<CaptureRequest>,
) -> AppResult<Json<Value>> {
    auth::require_session(&state.pool, &headers).await?;
    let mut tx = state.pool.begin().await?;
    let locked: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("osint:{article_id}"))
            .fetch_one(&mut *tx)
            .await?;
    if !locked {
        return Err(AppError::RateLimited("该材料正在取证".into()));
    }
    let row = sqlx::query(
        "SELECT a.url, a.title, f.title AS source_title, a.published_at FROM articles a
        JOIN feeds f ON f.id = a.feed_id LEFT JOIN source_feedback sf ON sf.feed_id = f.id
        WHERE a.id = $1 AND a.story_id = $2 AND NOT COALESCE(sf.muted, false)",
    )
    .bind(article_id)
    .bind(story_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    let recent: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM osint_captures WHERE article_id = $1 AND captured_at > now() - interval '1 minute')")
        .bind(article_id).fetch_one(&mut *tx).await?;
    if recent {
        return Err(AppError::RateLimited("每篇材料一分钟内只能取证一次".into()));
    }
    let url: String = row.get("url");
    checked_url(&url)?;
    // A separate client preserves response bytes and never uses reader cookies or browser sessions.
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .gzip(false)
        .brotli(false)
        .deflate(false)
        .build()?;
    let (body, mut details) =
        tokio::time::timeout(Duration::from_secs(20), fetch_evidence(&http, &url))
            .await
            .map_err(|_| AppError::Upstream("取证超时".into()))??;
    let captured_at = Utc::now();
    details["article_title"] = json!(row.get::<String, _>("title"));
    details["source_title"] = json!(row.get::<String, _>("source_title"));
    details["reported_published_at"] = json!(row.get::<Option<DateTime<Utc>>, _>("published_at"));
    details["archive"] = if request.include_archive {
        lookup_archive(&http, &url, row.get("published_at")).await
    } else {
        json!({"status": "not_requested"})
    };
    let sha256 = hex::encode(Sha256::digest(&body));
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO osint_captures (article_id, story_id, captured_at, sha256, body, details)
        VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(article_id)
    .bind(story_id)
    .bind(captured_at)
    .bind(&sha256)
    .bind(&body)
    .bind(&details)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"id": id, "captured_at": captured_at, "sha256": sha256, "byte_length": body.len(), "details": details}),
    ))
}

fn checked_url(raw: &str) -> AppResult<Url> {
    let url = Url::parse(raw).map_err(|_| AppError::BadRequest("材料 URL 无效".into()))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(AppError::BadRequest(
            "取证仅支持不含凭据的 HTTP(S) 地址".into(),
        ));
    }
    Ok(url)
}

async fn fetch_evidence(http: &reqwest::Client, url: &str) -> AppResult<(Vec<u8>, Value)> {
    let headers = HeaderMap::from_iter([
        (
            header::USER_AGENT,
            "rss-hub-reader/0.1 (OSINT evidence capture)"
                .parse()
                .unwrap(),
        ),
        (header::ACCEPT_ENCODING, "identity".parse().unwrap()),
    ]);
    let response = net::get_with_safe_redirects(http, url, &headers, false)
        .await
        .map_err(|_| {
            AppError::Upstream("无法读取公开材料：地址受限、网络失败或重定向无效".into())
        })?;
    capture_response(response, url).await
}

async fn capture_response(response: reqwest::Response, url: &str) -> AppResult<(Vec<u8>, Value)> {
    let final_url = checked_url(response.url().as_str())?;
    let status = response.status();
    let response_headers = [
        "content-type",
        "content-encoding",
        "etag",
        "last-modified",
        "date",
    ]
    .into_iter()
    .filter_map(|name| {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|v| (name.to_string(), json!(v)))
    })
    .collect::<serde_json::Map<_, _>>();
    let is_html = response_headers
        .get("content-type")
        .and_then(Value::as_str)
        .is_some_and(|v| {
            v.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("text/html")
        })
        && !response_headers.contains_key("content-encoding");
    let body = net::read_response_limited(response, MAX_BYTES)
        .await
        .map_err(|_| AppError::Upstream("响应读取失败或超过 1 MiB；未保存不完整证据".into()))?;
    let page = if is_html {
        page_metadata(&body, &final_url)
    } else {
        json!(null)
    };
    Ok((
        body,
        json!({"requested_url": url, "final_url": final_url.as_str(), "http_status": status.as_u16(),
        "outcome": if status.is_success() { "captured" } else { "http_error" }, "response_headers": response_headers, "page": page}),
    ))
}

fn page_metadata(body: &[u8], base: &Url) -> Value {
    let document = Html::parse_document(&String::from_utf8_lossy(body));
    let canonical = document
        .select(&Selector::parse("link[rel~='canonical']").unwrap())
        .next()
        .and_then(|node| node.value().attr("href"))
        .and_then(|href| base.join(href).ok())
        .filter(|url| checked_url(url.as_str()).is_ok())
        .map(|u| u.to_string());
    let published = document
        .select(
            &Selector::parse("meta[property='article:published_time'],meta[name='datePublished']")
                .unwrap(),
        )
        .next()
        .and_then(|node| node.value().attr("content"))
        .map(|v| v.chars().take(100).collect::<String>());
    let selector = Selector::parse("article a[href], main a[href]").unwrap();
    let mut seen = HashSet::new();
    let links = document.select(&selector).filter_map(|node| {
        let url = base.join(node.value().attr("href")?).ok()?;
        if checked_url(url.as_str()).is_err() || url.host_str() == base.host_str() || !seen.insert(url.to_string()) { return None; }
        Some(json!({"url": url.as_str(), "label": node.text().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ").chars().take(160).collect::<String>()}))
    }).take(20).collect::<Vec<_>>();
    json!({"declared_canonical_url": canonical, "declared_published_at": published, "outbound_links": links})
}

async fn lookup_archive(
    http: &reqwest::Client,
    url: &str,
    published_at: Option<DateTime<Utc>>,
) -> Value {
    let mut endpoint = Url::parse("https://archive.org/wayback/available").unwrap();
    endpoint.query_pairs_mut().append_pair("url", url);
    if let Some(time) = published_at {
        endpoint
            .query_pairs_mut()
            .append_pair("timestamp", &time.format("%Y%m%d%H%M%S").to_string());
    }
    let result = async {
        let response =
            net::get_with_safe_redirects(http, endpoint.as_str(), &HeaderMap::new(), false).await?;
        if !response.status().is_success() {
            return Err(AppError::Upstream("archive unavailable".into()));
        }
        let body = net::read_response_limited(response, 65_536).await?;
        let value: Value = serde_json::from_slice(&body).map_err(|e| AppError::Anyhow(e.into()))?;
        archive_result(&value)
    };
    match tokio::time::timeout(Duration::from_secs(10), result).await {
        Ok(Ok(value)) => value,
        _ => json!({"status": "unavailable", "checked_at": Utc::now()}),
    }
}

fn archive_result(value: &Value) -> AppResult<Value> {
    let snapshots = value
        .get("archived_snapshots")
        .and_then(Value::as_object)
        .ok_or_else(|| AppError::Upstream("archive response invalid".into()))?;
    let Some(closest) = snapshots.get("closest") else {
        return Ok(json!({"status": "not_found", "checked_at": Utc::now()}));
    };
    let mut url = checked_url(closest["url"].as_str().unwrap_or(""))?;
    let timestamp = closest["timestamp"].as_str().unwrap_or("");
    if closest["available"] != true
        || closest["status"] != "200"
        || url.host_str() != Some("web.archive.org")
        || url.port().is_some()
        || timestamp.len() != 14
        || !timestamp.bytes().all(|v| v.is_ascii_digit())
        || !url.path().starts_with(&format!("/web/{timestamp}/"))
    {
        return Err(AppError::Upstream("archive response invalid".into()));
    }
    let _ = url.set_scheme("https");
    Ok(
        json!({"status": "found", "url": url.as_str(), "timestamp": timestamp, "checked_at": Utc::now()}),
    )
}

async fn export_capture(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> AppResult<Response> {
    auth::require_session(&state.pool, &headers).await?;
    let row = sqlx::query("SELECT id, article_id, story_id, captured_at, sha256, body, details FROM osint_captures WHERE id = $1")
        .bind(id).fetch_optional(&state.pool).await?.ok_or(AppError::NotFound)?;
    let body: Vec<u8> = row.get("body");
    let export = json!({"format": "rss-osint-evidence-v1", "id": id, "article_id": row.get::<Uuid, _>("article_id"),
        "story_id": row.get::<Uuid, _>("story_id"), "captured_at": row.get::<DateTime<Utc>, _>("captured_at"),
        "sha256": row.get::<String, _>("sha256"), "byte_length": body.len(), "body_hex": hex::encode(&body),
        "details": row.get::<Value, _>("details")});
    Ok((
        [
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"osint-{id}.json\""),
            ),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        Json(export),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(publisher: &str, url: &str, hash: &str) -> Source {
        Source {
            article_id: Uuid::new_v4(),
            title: "Report".into(),
            source_title: publisher.into(),
            publisher: publisher.into(),
            url: url.into(),
            canonical_url: canonicalize_url(url),
            content_hash: hash.into(),
            reported_published_at: None,
            fetched_at: Utc::now(),
            dependency_group: 0,
            latest_capture: None,
        }
    }

    #[test]
    fn traces_transitive_dependencies_and_counts_only_core_support() {
        let mut sources = vec![
            source("agency", "https://agency.test/1", "a"),
            source("mirror", "https://mirror.test/1", "b"),
            source("agency", "https://mirror.test/1?utm_source=rss", "b"),
            source("reporter", "https://reporter.test/1", ""),
            source("another", "https://another.test/1", ""),
        ];
        assert_eq!(group_sources(&mut sources), 3);
        assert_eq!(sources[0].dependency_group, sources[1].dependency_group);
        let citation = |index: usize, claim: &str, verified| GeneratedCitation {
            claim_id: claim.into(),
            evidence_id: "E1".into(),
            article_id: sources[index].article_id,
            source_title: "Report".into(),
            article_title: "Report".into(),
            url: sources[index].url.clone(),
            excerpt: "Evidence".into(),
            verified,
        };
        let mut citations = vec![
            citation(0, "summary", true),
            citation(1, "summary", true),
            citation(3, "bullet:0", true),
            citation(4, "summary", false),
        ];
        assert_eq!(core_support(&sources, &citations), 1);
        citations.push(citation(3, "summary", true));
        assert_eq!(core_support(&sources, &citations), 2);
    }

    #[test]
    fn page_and_archive_metadata_are_untrusted_and_bounded() {
        let body = br#"<link rel="canonical" href="/original"><meta property="article:published_time" content="2026-09-08T00:00:00Z">
            <article><a href="https://record.test/1">Original record</a><a href="javascript:alert(1)">Bad</a>
            <a href="https://user:secret@record.test/2">Bad</a><a href="/same-site">Local</a>
            <a href="https://record.test/1">Duplicate</a></article>"#;
        let page = page_metadata(body, &Url::parse("https://news.test/story").unwrap());
        assert_eq!(page["declared_canonical_url"], "https://news.test/original");
        assert_eq!(page["outbound_links"].as_array().unwrap().len(), 1);
        assert!(checked_url("file:///etc/passwd").is_err());
        assert!(checked_url("https://user:secret@example.com").is_err());
        assert_eq!(
            archive_result(&json!({"archived_snapshots": {}})).unwrap()["status"],
            "not_found"
        );
        assert!(archive_result(&json!({"error": "rate limited"})).is_err());
        let mut archive = json!({"archived_snapshots": {"closest": {"available": true, "status": "200",
            "url": "http://web.archive.org/web/20260908000000/https://news.test/story", "timestamp": "20260908000000"}}});
        assert!(
            archive_result(&archive).unwrap()["url"]
                .as_str()
                .unwrap()
                .starts_with("https://web.archive.org/")
        );
        archive["archived_snapshots"]["closest"]["url"] =
            json!("https://evil.test/web/20260908000000/https://news.test/story");
        assert!(archive_result(&archive).is_err());
    }

    #[tokio::test]
    async fn capture_preserves_error_bytes_and_rejects_private_or_oversized_responses() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route(
                        "/",
                        get(|| async {
                            (
                                axum::http::StatusCode::FORBIDDEN,
                                [
                                    (header::CONTENT_TYPE, "text/html"),
                                    (header::SET_COOKIE, "private=value"),
                                ],
                                "<html>Denied</html>",
                            )
                        }),
                    )
                    .route("/large", get(|| async { vec![b'x'; MAX_BYTES + 1] })),
            )
            .await
            .unwrap();
        });
        let http = reqwest::Client::new();
        assert!(fetch_evidence(&http, &url).await.is_err());
        let response = http.get(&url).send().await.unwrap();
        let (body, record) = capture_response(response, &url).await.unwrap();
        assert_eq!(body, b"<html>Denied</html>");
        assert_eq!(record["http_status"], 403);
        assert_eq!(record["outcome"], "http_error");
        assert!(record["response_headers"].get("set-cookie").is_none());
        let response = http.get(format!("{url}large")).send().await.unwrap();
        assert!(capture_response(response, &url).await.is_err());
        server.abort();
    }

    // Requires a disposable database with migrations applied; no worker or LLM is started.
    #[tokio::test]
    #[ignore = "requires RSS_OSINT_TEST_DATABASE_URL and public HTTPS connectivity"]
    async fn evidence_roundtrip_with_postgres_and_public_page() {
        use std::sync::Arc;
        use tokio::sync::{Semaphore, mpsc};
        let pool = sqlx::PgPool::connect(&std::env::var("RSS_OSINT_TEST_DATABASE_URL").unwrap())
            .await
            .unwrap();
        let config = crate::config::Config::from_env().unwrap();
        let index = tempfile::tempdir().unwrap();
        let (worker_tx, _worker_rx) = mpsc::channel(1);
        let state = AppState {
            config,
            pool: pool.clone(),
            http: reqwest::Client::new(),
            search: Arc::new(crate::search::SearchIndex::open(index.path()).unwrap()),
            worker_tx,
            login_limiter: Arc::new(crate::security::LoginRateLimiter::default()),
            opencli_semaphore: Arc::new(Semaphore::new(1)),
            llm_semaphore: Arc::new(Semaphore::new(1)),
            translation_semaphore: Arc::new(Semaphore::new(1)),
        };
        let story: Uuid = sqlx::query_scalar("INSERT INTO story_clusters (representative_title, story_key) VALUES ('OSINT fixture', 'osint-fixture') RETURNING id")
            .fetch_one(&pool).await.unwrap();
        let feed: Uuid = sqlx::query_scalar(
            "INSERT INTO feeds (title, feed_url) VALUES ('OSINT fixture', $1) RETURNING id",
        )
        .bind(format!("https://example.com/{}", Uuid::new_v4()))
        .fetch_one(&pool)
        .await
        .unwrap();
        let article: Uuid = sqlx::query_scalar("INSERT INTO articles (feed_id, guid, url, url_hash, title, canonical_url, content_hash, story_id)
            VALUES ($1, 'osint-fixture', 'https://1.1.1.1/cdn-cgi/trace', 'test-url-hash', 'Public HTTPS fixture', 'https://1.1.1.1/cdn-cgi/trace', 'body-hash', $2) RETURNING id")
            .bind(feed).bind(story).fetch_one(&pool).await.unwrap();
        let token = Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO sessions (token_hash, username, expires_at) VALUES ($1, 'fixture', now() + interval '1 hour')")
            .bind(hex::encode(Sha256::digest(token.as_bytes()))).execute(&pool).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, router().with_state(state))
                .await
                .unwrap();
        });
        let http = reqwest::Client::new();
        let cookie = format!("rss_session={token}");
        let path = format!("{base}/api/v2/events/{story}/osint");
        assert_eq!(http.get(&path).send().await.unwrap().status(), 401);
        let dossier: Value = http
            .get(&path)
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(dossier["dependency_groups"], 1);
        assert!(dossier["sources"][0]["reported_published_at"].is_null());
        let capture_path = format!("{base}/api/v2/events/{story}/evidence/{article}/capture");
        let response = http
            .post(&capture_path)
            .header(header::COOKIE, &cookie)
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        let status = response.status();
        let captured: Value = response.json().await.unwrap();
        assert!(status.is_success(), "{captured}");
        assert_eq!(captured["details"]["archive"]["status"], "not_requested");
        assert_eq!(captured["details"]["http_status"], 200);
        let export_url = format!(
            "{base}/api/v2/osint/captures/{}/export",
            captured["id"].as_str().unwrap()
        );
        assert_eq!(http.get(&export_url).send().await.unwrap().status(), 401);
        let response = http
            .get(&export_url)
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        assert!(
            response.headers()[header::CONTENT_DISPOSITION]
                .to_str()
                .unwrap()
                .starts_with("attachment;")
        );
        let export: Value = response.json().await.unwrap();
        let bytes = hex::decode(export["body_hex"].as_str().unwrap()).unwrap();
        assert_eq!(hex::encode(Sha256::digest(&bytes)), export["sha256"]);
        assert_eq!(
            bytes.len(),
            export["byte_length"].as_u64().unwrap() as usize
        );
        assert_eq!(
            http.post(&capture_path)
                .header(header::COOKIE, &cookie)
                .json(&json!({}))
                .send()
                .await
                .unwrap()
                .status(),
            429
        );
        let reloaded: Value = http
            .get(&path)
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            reloaded["sources"][0]["latest_capture"]["id"],
            captured["id"]
        );
        sqlx::query("DELETE FROM feeds WHERE id = $1")
            .bind(feed)
            .execute(&pool)
            .await
            .unwrap();
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM osint_captures WHERE article_id = $1")
                .bind(article)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 0);
        sqlx::query("DELETE FROM story_clusters WHERE id = $1")
            .bind(story)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(hex::encode(Sha256::digest(token.as_bytes())))
            .execute(&pool)
            .await
            .unwrap();
        server.abort();
        pool.close().await;
    }
}
