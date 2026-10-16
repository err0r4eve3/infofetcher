use std::collections::HashSet;

use chrono::{DateTime, Utc};
use reqwest::Url;
use serde::Serialize;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::{error::AppResult, extractor::clean_article_text, feeds::canonicalize_url};

const LIMIT: i64 = 30;

#[derive(Debug, Serialize)]
pub(super) struct EventEvidence {
    article_id: Uuid,
    source_title: String,
    title: String,
    url: String,
    excerpt: String,
    quality_score: i32,
    // Retained for older clients; new clients use the explicit nullable publication time.
    published_at: DateTime<Utc>,
    reported_published_at: Option<DateTime<Utc>>,
    fetched_at: DateTime<Utc>,
    provenance: Provenance,
    collection: Collection,
}

#[derive(Debug, Serialize)]
struct Provenance {
    link_host: Option<String>,
    feed_host: Option<String>,
    link_is_aggregator: bool,
    // Only repeated URL/body fingerprints, never an inferred original publisher.
    duplicate_group: Option<usize>,
}

#[derive(Debug, Serialize)]
struct Collection {
    feed_id: Uuid,
    last_success_at: Option<DateTime<Utc>>,
    consecutive_failures: i32,
}

#[derive(Debug, Serialize)]
pub(super) struct EvidenceScope {
    total_articles: i64,
    returned_articles: usize,
    duplicate_groups: usize,
    feeds_with_failures: usize,
    feeds_never_succeeded: usize,
}

#[derive(Default)]
struct Fingerprint {
    canonical: String,
    body: String,
}

pub(super) async fn load(
    pool: &PgPool,
    story_id: Uuid,
) -> AppResult<(Vec<EventEvidence>, EvidenceScope)> {
    let rows = sqlx::query(
        "SELECT a.id, a.title, a.url, a.canonical_url, a.content_hash, a.url_hash,
                COALESCE(NULLIF(a.extracted_content, ''), a.content) AS body,
                a.quality_score, a.published_at, a.fetched_at,
                f.id AS feed_id, f.title AS source_title, f.feed_url,
                f.last_success_at, f.consecutive_failures, COUNT(*) OVER () AS total_articles
         FROM articles a
         JOIN feeds f ON f.id = a.feed_id
         LEFT JOIN source_feedback sf ON sf.feed_id = f.id
         WHERE a.story_id = $1 AND COALESCE(sf.muted, false) = false
         ORDER BY a.quality_score DESC, COALESCE(a.published_at, a.fetched_at) DESC, a.id
         LIMIT $2",
    )
    .bind(story_id)
    .bind(LIMIT)
    .fetch_all(pool)
    .await?;
    let total_articles = rows.first().map_or(0, |row| row.get("total_articles"));
    let mut fingerprints = Vec::with_capacity(rows.len());
    let mut evidence = Vec::with_capacity(rows.len());
    for row in rows {
        let body: String = row.get("body");
        let title: String = row.get("title");
        let url: String = row.get("url");
        let content_hash: String = row.get("content_hash");
        fingerprints.push(fingerprint(
            &row.get::<String, _>("canonical_url"),
            &url,
            &body,
            &content_hash,
            &row.get::<String, _>("url_hash"),
        ));
        let link_host = web_host(&url);
        let reported_published_at: Option<DateTime<Utc>> = row.get("published_at");
        let fetched_at = row.get("fetched_at");
        evidence.push(EventEvidence {
            article_id: row.get("id"),
            source_title: row.get("source_title"),
            title: title.clone(),
            url,
            excerpt: super::plain_excerpt(&clean_article_text(&body, &title), 220),
            quality_score: row.get("quality_score"),
            published_at: reported_published_at.unwrap_or(fetched_at),
            reported_published_at,
            fetched_at,
            provenance: Provenance {
                link_is_aggregator: link_host.as_deref().is_some_and(is_aggregator),
                link_host,
                feed_host: web_host(&row.get::<String, _>("feed_url")),
                duplicate_group: None,
            },
            collection: Collection {
                feed_id: row.get("feed_id"),
                last_success_at: row.get("last_success_at"),
                consecutive_failures: row.get("consecutive_failures"),
            },
        });
    }
    let duplicates = duplicate_groups(&fingerprints);
    for (item, group) in evidence.iter_mut().zip(&duplicates) {
        item.provenance.duplicate_group = *group;
    }
    let scope = EvidenceScope {
        total_articles,
        returned_articles: evidence.len(),
        duplicate_groups: duplicates.into_iter().flatten().max().unwrap_or(0),
        feeds_with_failures: evidence
            .iter()
            .filter(|item| item.collection.consecutive_failures > 0)
            .map(|item| item.collection.feed_id)
            .collect::<HashSet<_>>()
            .len(),
        feeds_never_succeeded: evidence
            .iter()
            .filter(|item| item.collection.last_success_at.is_none())
            .map(|item| item.collection.feed_id)
            .collect::<HashSet<_>>()
            .len(),
    };
    Ok((evidence, scope))
}

fn web_host(value: &str) -> Option<String> {
    let url = Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    url.host_str()
        .map(|host| host.strip_prefix("www.").unwrap_or(host).to_owned())
}

fn is_aggregator(host: &str) -> bool {
    matches!(
        host,
        "news.google.com"
            | "feedproxy.google.com"
            | "feeds.feedburner.com"
            | "feeds2.feedburner.com"
    )
}

fn fingerprint(canonical: &str, url: &str, body: &str, hash: &str, url_hash: &str) -> Fingerprint {
    let canonical = if web_host(canonical).is_some() {
        canonical
    } else {
        url
    };
    Fingerprint {
        canonical: if web_host(canonical).is_some() {
            canonicalize_url(canonical)
        } else {
            String::new()
        },
        // Older rows were backfilled with the URL hash; empty content is not a shared report.
        body: if !body.trim().is_empty() && hash != url_hash {
            hash.to_owned()
        } else {
            String::new()
        },
    }
}

fn duplicate_groups(keys: &[Fingerprint]) -> Vec<Option<usize>> {
    let mut visited = vec![false; keys.len()];
    let mut result = vec![None; keys.len()];
    let mut group = 0;
    // Bounded to the 30 returned materials. Transitive matches retain every material for review.
    for start in 0..keys.len() {
        if visited[start] {
            continue;
        }
        visited[start] = true;
        let mut members = vec![start];
        let mut pending = vec![start];
        while let Some(index) = pending.pop() {
            for next in 0..keys.len() {
                if visited[next] {
                    continue;
                }
                let a = &keys[index];
                let b = &keys[next];
                if (!a.canonical.is_empty() && a.canonical == b.canonical)
                    || (!a.body.is_empty() && a.body == b.body)
                {
                    visited[next] = true;
                    members.push(next);
                    pending.push(next);
                }
            }
        }
        if members.len() > 1 {
            group += 1;
            for index in members {
                result[index] = Some(group);
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_identify_hosts_not_original_publishers() {
        assert_eq!(
            web_host("https://WWW.Example.org/news"),
            Some("example.org".into())
        );
        assert_eq!(web_host("local://document/1"), None);
        assert_eq!(web_host("not a URL"), None);
        assert!(is_aggregator("news.google.com"));
        assert!(!is_aggregator("news.google.com.example.org"));
        assert!(!is_aggregator("example.org"));
    }

    #[test]
    fn duplicate_clues_connect_urls_and_bodies_transitively() {
        let keys = [
            fingerprint("", "https://a.test/1?utm_source=rss", "text A", "a", "url1"),
            fingerprint("", "https://b.test/2", "text B", "b", "url2"),
            fingerprint(
                "https://a.test/1",
                "https://c.test/3",
                "text B",
                "b",
                "url3",
            ),
            fingerprint("", "https://a.test/4", "text C", "c", "url4"),
        ];
        assert_eq!(
            duplicate_groups(&keys),
            vec![Some(1), Some(1), Some(1), None]
        );
    }

    #[test]
    fn unknown_links_empty_bodies_and_legacy_hashes_do_not_create_duplicates() {
        let keys = [
            fingerprint("", "invalid", "", "empty", "a"),
            fingerprint("", "invalid", "  ", "empty", "b"),
            fingerprint("", "https://a.test/1", "body", "legacy", "legacy"),
            fingerprint("", "https://b.test/2", "body", "legacy", "legacy"),
        ];
        assert_eq!(duplicate_groups(&keys), vec![None; 4]);
    }

    #[tokio::test]
    #[ignore = "requires RSS_OSINT_TEST_DATABASE_URL pointing to a disposable migrated database"]
    async fn evidence_scope_roundtrip_with_postgres() {
        let pool = PgPool::connect(&std::env::var("RSS_OSINT_TEST_DATABASE_URL").unwrap())
            .await
            .unwrap();
        let story = Uuid::new_v4();
        let feeds = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
        sqlx::query("INSERT INTO story_clusters (id, representative_title, story_key) VALUES ($1, 'Evidence scope test', $1::text)")
            .bind(story).execute(&pool).await.unwrap();
        for (index, id) in feeds.iter().enumerate() {
            sqlx::query("INSERT INTO feeds (id, title, feed_url, last_success_at, consecutive_failures) VALUES ($1, 'Test feed', $2, $3, $4)")
                .bind(id).bind(format!("https://feeds.test/{id}"))
                .bind(if index == 0 { None } else { Some(Utc::now()) })
                .bind(if index == 0 { 2 } else { 0 }).execute(&pool).await.unwrap();
        }
        sqlx::query("INSERT INTO source_feedback (feed_id, muted) VALUES ($1, true)")
            .bind(feeds[2])
            .execute(&pool)
            .await
            .unwrap();
        for index in 0..34 {
            let feed_id = if index >= 32 {
                feeds[2]
            } else if index == 1 {
                feeds[1]
            } else {
                feeds[0]
            };
            let url = if index == 0 {
                "https://news.google.com/articles/test".to_owned()
            } else {
                format!("https://publisher.test/{index}")
            };
            sqlx::query("INSERT INTO articles (feed_id, guid, url, url_hash, canonical_url, content_hash, title, content, story_id, quality_score, published_at)
                VALUES ($1, $2, $3, $2, $3, $4, 'Test article', 'Shared body', $5, $6, $7)")
                .bind(feed_id).bind(index.to_string()).bind(&url)
                .bind(if index < 2 { "shared-body".to_owned() } else { format!("body-{index}") })
                .bind(story).bind(100 - index).bind(if index == 0 { None } else { Some(Utc::now()) })
                .execute(&pool).await.unwrap();
        }
        let (items, scope) = load(&pool, story).await.unwrap();
        assert_eq!(scope.total_articles, 32);
        assert_eq!(scope.returned_articles, 30);
        assert_eq!(scope.duplicate_groups, 1);
        assert_eq!(scope.feeds_with_failures, 1);
        assert_eq!(scope.feeds_never_succeeded, 1);
        assert!(items.iter().all(|item| item.collection.feed_id != feeds[2]));
        assert_eq!(items[0].provenance.duplicate_group, Some(1));
        assert_eq!(items[1].provenance.duplicate_group, Some(1));
        let json = serde_json::to_value(&items[0]).unwrap();
        assert!(json["reported_published_at"].is_null());
        assert_eq!(json["published_at"], json["fetched_at"]);
        assert_eq!(json["provenance"]["link_host"], "news.google.com");
        assert_eq!(json["provenance"]["feed_host"], "feeds.test");
        assert_eq!(json["provenance"]["link_is_aggregator"], true);
        assert_eq!(json["collection"]["consecutive_failures"], 2);
        let (empty, empty_scope) = load(&pool, Uuid::new_v4()).await.unwrap();
        assert!(empty.is_empty());
        assert_eq!(empty_scope.total_articles, 0);
        assert_eq!(empty_scope.duplicate_groups, 0);
        sqlx::query("DELETE FROM feeds WHERE id = ANY($1)")
            .bind(&feeds[..])
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM story_clusters WHERE id = $1")
            .bind(story)
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    }
}
