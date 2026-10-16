use std::io::Cursor;

use chrono::{DateTime, Utc};
use feed_rs::parser;
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use scraper::{Html, Selector};
use sha2::{Digest, Sha256};

use crate::{
    error::{AppError, AppResult},
    net::{get_with_safe_redirects, read_response_limited},
};

#[derive(Debug, Clone)]
pub struct ParsedFeed {
    pub title: String,
    pub site_url: Option<String>,
    pub language: Option<String>,
    pub websub_hub_url: Option<String>,
    pub websub_topic_url: Option<String>,
    pub ttl_minutes: Option<i32>,
    pub items: Vec<ParsedFeedItem>,
}

#[derive(Debug, Clone)]
pub struct ParsedFeedItem {
    pub guid: String,
    pub title: String,
    pub url: String,
    pub author: Option<String>,
    pub content: String,
    pub published_at: Option<DateTime<Utc>>,
}

pub async fn discover_feed_url(
    http: &reqwest::Client,
    raw_url: &str,
    allow_private: bool,
    max_bytes: usize,
) -> AppResult<String> {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static("infofetcher/0.1"));
    let response = get_with_safe_redirects(http, raw_url, &headers, allow_private).await?;
    if !response.status().is_success() {
        return Err(AppError::Upstream(format!(
            "订阅发现请求失败: {}",
            response.status()
        )));
    }
    let bytes = read_response_limited(response, max_bytes).await?;
    if parse_feed_bytes(&bytes, raw_url).is_ok() {
        return Ok(raw_url.to_string());
    }

    let html = String::from_utf8_lossy(&bytes);
    let base = reqwest::Url::parse(raw_url)
        .map_err(|err| AppError::BadRequest(format!("URL 无效: {err}")))?;
    for candidate in feed_candidates_from_html(&html, &base) {
        let response = get_with_safe_redirects(http, &candidate, &headers, allow_private).await?;
        if !response.status().is_success() {
            continue;
        }
        let bytes = read_response_limited(response, max_bytes).await?;
        if parse_feed_bytes(&bytes, &candidate).is_ok() {
            return Ok(candidate);
        }
    }
    Err(AppError::BadRequest(
        "该页面没有可验证的 RSS、Atom 或 JSON Feed".to_string(),
    ))
}

fn feed_candidates_from_html(html: &str, base: &reqwest::Url) -> Vec<String> {
    let document = Html::parse_document(html);
    let Ok(selector) = Selector::parse("link[rel~='alternate']") else {
        return Vec::new();
    };
    document
        .select(&selector)
        .filter_map(|element| {
            let media_type = element.value().attr("type")?.to_ascii_lowercase();
            if !matches!(
                media_type.as_str(),
                "application/rss+xml"
                    | "application/atom+xml"
                    | "application/feed+json"
                    | "application/json"
            ) {
                return None;
            }
            let href = element.value().attr("href")?;
            base.join(href).ok().map(|url| url.to_string())
        })
        .collect()
}

pub fn parse_feed_bytes(bytes: &[u8], fallback_url: &str) -> AppResult<ParsedFeed> {
    let feed = parser::parse(Cursor::new(bytes))
        .map_err(|err| AppError::BadRequest(format!("Feed 解析失败: {err}")))?;
    let title = feed
        .title
        .map(|title| title.content)
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| fallback_url.to_string());
    let site_url =
        preferred_link(&feed.links, &["alternate"]).or_else(|| preferred_link(&feed.links, &[""]));
    let websub_hub_url = preferred_link(&feed.links, &["hub"]);
    let websub_topic_url = preferred_link(&feed.links, &["self"]);
    let language = feed.language.clone();
    let ttl_minutes = feed.ttl.and_then(|value| i32::try_from(value).ok());
    let items = feed
        .entries
        .into_iter()
        .filter_map(|entry| {
            let url = preferred_link(&entry.links, &["alternate", "canonical"])
                .or_else(|| preferred_link(&entry.links, &[""]))?;
            let title = entry
                .title
                .map(|title| title.content)
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| url.clone());
            let author = entry.authors.first().map(|author| author.name.clone());
            let summary = entry
                .summary
                .map(|summary| summary.content)
                .unwrap_or_default();
            let content = entry
                .content
                .and_then(|content| content.body)
                .filter(|body| !body.trim().is_empty())
                .unwrap_or(summary);
            let content = clean_feed_content(&content);
            Some(ParsedFeedItem {
                guid: if entry.id.trim().is_empty() {
                    url.clone()
                } else {
                    entry.id
                },
                title,
                url,
                author,
                content,
                published_at: entry.published.or(entry.updated),
            })
        })
        .collect();
    Ok(ParsedFeed {
        title,
        site_url,
        language,
        websub_hub_url,
        websub_topic_url,
        ttl_minutes,
        items,
    })
}

fn preferred_link(links: &[feed_rs::model::Link], relationships: &[&str]) -> Option<String> {
    relationships.iter().find_map(|relationship| {
        links
            .iter()
            .filter(|link| {
                let rel = link.rel.as_deref().unwrap_or("");
                rel.eq_ignore_ascii_case(relationship)
            })
            .filter(|link| {
                link.media_type.as_deref().is_none_or(|media_type| {
                    !media_type.starts_with("audio/")
                        && !media_type.starts_with("video/")
                        && !media_type.starts_with("image/")
                })
            })
            .map(|link| link.href.trim().to_string())
            .find(|href| href.starts_with("http://") || href.starts_with("https://"))
    })
}

fn clean_feed_content(content: &str) -> String {
    if content.contains('<') && content.contains('>') {
        let fragment = Html::parse_fragment(content);
        let text = fragment.root_element().text().collect::<Vec<_>>().join(" ");
        return normalize_whitespace(&text);
    }

    normalize_whitespace(content)
}

pub fn hash_url(url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(canonicalize_url(url).as_bytes());
    hex::encode(hasher.finalize())
}

pub fn canonicalize_url(url: &str) -> String {
    let Ok(mut parsed) = reqwest::Url::parse(url) else {
        return url.to_string();
    };
    parsed.set_fragment(None);
    let mut pairs = parsed
        .query_pairs()
        .filter(|(key, _)| !is_tracking_query_parameter(key))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    pairs.sort_unstable();
    parsed.set_query(None);
    if !pairs.is_empty() {
        let mut query = parsed.query_pairs_mut();
        for (key, value) in pairs {
            query.append_pair(&key, &value);
        }
    }
    parsed.to_string()
}

fn is_tracking_query_parameter(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.starts_with("utm_")
        || matches!(
            key.as_str(),
            "fbclid"
                | "gclid"
                | "dclid"
                | "msclkid"
                | "mc_cid"
                | "mc_eid"
                | "igshid"
                | "_ga"
                | "li_fat_id"
                | "ttclid"
                | "twclid"
                | "vero_id"
        )
}

pub fn excerpt(text: &str, max_chars: usize) -> String {
    let normalized = normalize_whitespace(text);
    let mut chars = normalized.chars();
    let excerpt = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{excerpt}...")
    } else {
        excerpt
    }
}

fn normalize_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_urls_drop_tracking_parameters() {
        assert_eq!(
            canonicalize_url("https://example.com/a?b=2&utm_source=x&_ga=1&ttclid=t&twclid=w&li_fat_id=l&vero_id=v&a=1#frag"),
            "https://example.com/a?a=1&b=2"
        );
    }

    #[test]
    fn parses_basic_rss_feed() {
        let xml = br#"<?xml version="1.0"?>
        <rss version="2.0"><channel><title>Example</title><link>https://example.com</link>
          <item><title>Hello</title><guid>1</guid><link>https://example.com/1</link><description>World</description></item>
        </channel></rss>"#;
        let parsed = parse_feed_bytes(xml, "https://example.com/feed.xml").unwrap();
        assert_eq!(parsed.title, "Example");
        assert_eq!(parsed.items.len(), 1);
        assert_eq!(parsed.items[0].title, "Hello");
    }

    #[test]
    fn cleans_html_content_from_atom_feed() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
        <feed xmlns="http://www.w3.org/2005/Atom">
          <title>V2EX</title>
          <entry>
            <title>HTML content</title>
            <link rel="alternate" type="text/html" href="https://www.v2ex.com/t/1#reply1" />
            <id>tag:www.v2ex.com,2026-04-27:/t/1</id>
            <content type="html"><![CDATA[
              <h2>背景</h2><p>第一段<br/>第二段 &amp; 结尾</p>
            ]]></content>
          </entry>
        </feed>"#;

        let parsed = parse_feed_bytes(xml.as_bytes(), "https://www.v2ex.com/index.xml").unwrap();

        assert_eq!(parsed.items[0].content, "背景 第一段 第二段 & 结尾");
        assert!(!parsed.items[0].content.contains("<p>"));
        assert!(!parsed.items[0].content.contains("<br"));
    }

    #[test]
    fn url_hash_is_stable() {
        assert_eq!(
            hash_url("https://example.com/a"),
            hash_url("https://example.com/a")
        );
        assert_ne!(
            hash_url("https://example.com/a"),
            hash_url("https://example.com/b")
        );
        assert_eq!(
            hash_url("https://www.v2ex.com/t/1208778#reply1"),
            hash_url("https://www.v2ex.com/t/1208778#reply9")
        );
        assert_eq!(
            hash_url("https://example.com/post?utm_source=rss&a=1&b=2"),
            hash_url("https://example.com/post?b=2&a=1")
        );
    }

    #[test]
    fn parses_json_feed_and_prefers_article_url() {
        let json = r#"{
          "version":"https://jsonfeed.org/version/1.1",
          "title":"JSON Feed",
          "home_page_url":"https://example.com/",
          "feed_url":"https://example.com/feed.json",
          "language":"zh-CN",
          "items":[{
            "id":"post-1",
            "url":"https://example.com/posts/1",
            "title":"JSON item",
            "content_text":"正文"
          }]
        }"#;
        let parsed = parse_feed_bytes(json.as_bytes(), "https://example.com/feed.json").unwrap();
        assert_eq!(parsed.language.as_deref(), Some("zh-CN"));
        assert_eq!(parsed.items[0].url, "https://example.com/posts/1");
    }

    #[test]
    fn discovers_websub_links() {
        let xml = br#"<?xml version="1.0"?>
        <feed xmlns="http://www.w3.org/2005/Atom">
          <title>Push Feed</title>
          <link rel="alternate" href="https://example.com/" />
          <link rel="self" href="https://example.com/feed.xml" />
          <link rel="hub" href="https://hub.example.com/" />
          <entry><title>One</title><id>one</id><link rel="alternate" href="https://example.com/one" /></entry>
        </feed>"#;
        let parsed = parse_feed_bytes(xml, "https://example.com/feed.xml").unwrap();
        assert_eq!(parsed.site_url.as_deref(), Some("https://example.com/"));
        assert_eq!(
            parsed.websub_topic_url.as_deref(),
            Some("https://example.com/feed.xml")
        );
        assert_eq!(
            parsed.websub_hub_url.as_deref(),
            Some("https://hub.example.com/")
        );
    }

    #[test]
    fn discovers_feed_links_from_html() {
        let base = reqwest::Url::parse("https://example.com/blog/").unwrap();
        let candidates = feed_candidates_from_html(
            r#"<link rel="alternate" type="application/atom+xml" href="../feed.xml">"#,
            &base,
        );
        assert_eq!(candidates, vec!["https://example.com/feed.xml"]);
    }
}
