use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use rss::{ChannelBuilder, GuidBuilder, ItemBuilder};
use scraper::{Html, Selector};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::{process::Command, sync::Semaphore, time};

use crate::{
    config::Config,
    error::{AppError, AppResult},
    feeds::parse_feed_bytes,
    net::{get_with_safe_redirects, read_response_limited, validate_http_url},
};

#[derive(Debug, Clone)]
pub struct HubFeed {
    pub title: String,
    pub link: String,
    pub description: String,
    pub items: Vec<HubItem>,
}

#[derive(Debug, Clone)]
pub struct HubItem {
    pub title: String,
    pub link: String,
    pub guid: String,
    pub description: String,
    pub published_at: Option<DateTime<Utc>>,
}

#[async_trait]
pub trait HubAdapter: Send + Sync {
    async fn generate(
        &self,
        config: &Config,
        http: &reqwest::Client,
        path: &str,
    ) -> AppResult<HubFeed>;
}

pub async fn generate_feed(
    config: &Config,
    http: &reqwest::Client,
    adapter: &str,
    path: &str,
) -> AppResult<HubFeed> {
    match adapter {
        "github-releases" => GithubReleases.generate(config, http, path).await,
        "hn-top" => HackerNewsTop.generate(config, http, path).await,
        "opencli" => OpenCli.generate(config, http, path).await,
        "activitypub" => ActivityPub.generate(config, http, path).await,
        "rsshub" => RssHub.generate(config, http, path).await,
        "web-watch" => WebWatch.generate(config, http, path).await,
        "echo" => Echo.generate(config, http, path).await,
        _ => Err(AppError::NotFound),
    }
}

pub async fn generate_feed_bounded(
    config: &Config,
    http: &reqwest::Client,
    opencli_semaphore: &Semaphore,
    adapter: &str,
    path: &str,
) -> AppResult<HubFeed> {
    let _opencli_permit = if adapter == "opencli" {
        Some(
            opencli_semaphore
                .acquire()
                .await
                .map_err(|_| AppError::RateLimited("OpenCLI 当前不可用".to_string()))?,
        )
    } else {
        None
    };
    generate_feed(config, http, adapter, path).await
}

pub fn normalize_social_source(
    config: &Config,
    adapter: &str,
    path: &str,
) -> AppResult<(String, String)> {
    let adapter = adapter.trim().to_ascii_lowercase();
    let path = path.trim();
    match adapter.as_str() {
        "opencli" => {
            let name = path.trim_matches('/');
            if name.is_empty() || name.contains('/') || config.opencli_feed(name).is_none() {
                return Err(AppError::BadRequest(
                    "OpenCLI 来源必须是服务端白名单中的名称".to_string(),
                ));
            }
            Ok((adapter, name.to_string()))
        }
        "activitypub" => Ok((adapter, normalize_activitypub_source(path)?)),
        "rsshub" => {
            if !config.rsshub_enabled() {
                return Err(AppError::BadRequest("RSSHUB_BASE_URL 未配置".to_string()));
            }
            let route = path.trim_matches('/');
            if route.is_empty()
                || route.contains("://")
                || route.contains('\\')
                || route.contains('#')
                || route.chars().any(char::is_control)
                || route
                    .split(['/', '?'])
                    .any(|segment| matches!(segment, "." | ".."))
            {
                return Err(AppError::BadRequest("RSSHub route 无效".to_string()));
            }
            Ok((adapter, route.to_string()))
        }
        _ => Err(AppError::BadRequest(
            "社媒来源仅支持 opencli、activitypub 或 rsshub".to_string(),
        )),
    }
}

pub fn social_feed_url(app_public_url: &str, adapter: &str, path: &str) -> AppResult<String> {
    let raw = format!(
        "{}/hub/{}/{}",
        app_public_url.trim_end_matches('/'),
        adapter,
        path.trim_start_matches('/')
    );
    let url = reqwest::Url::parse(&raw)
        .map_err(|err| AppError::BadRequest(format!("社媒订阅 URL 无效: {err}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(AppError::BadRequest(
            "APP_PUBLIC_URL 必须使用 http 或 https".to_string(),
        ));
    }
    Ok(url.to_string())
}

pub fn render_rss(feed: HubFeed) -> String {
    let items = feed
        .items
        .into_iter()
        .map(|item| {
            ItemBuilder::default()
                .title(Some(item.title))
                .link(Some(item.link.clone()))
                .guid(Some(
                    GuidBuilder::default()
                        .value(item.guid)
                        .permalink(false)
                        .build(),
                ))
                .description(Some(item.description))
                .pub_date(item.published_at.map(|date| date.to_rfc2822()))
                .build()
        })
        .collect::<Vec<_>>();

    ChannelBuilder::default()
        .title(feed.title)
        .link(feed.link)
        .description(feed.description)
        .items(items)
        .build()
        .to_string()
}

struct Echo;

#[async_trait]
impl HubAdapter for Echo {
    async fn generate(
        &self,
        _config: &Config,
        _http: &reqwest::Client,
        path: &str,
    ) -> AppResult<HubFeed> {
        let label = if path.trim().is_empty() {
            "demo"
        } else {
            path.trim_matches('/')
        };
        Ok(HubFeed {
            title: format!("Echo {label}"),
            link: "https://example.com".to_string(),
            description: "Infofetcher echo adapter".to_string(),
            items: vec![HubItem {
                title: format!("Echo item for {label}"),
                link: "https://example.com/echo".to_string(),
                guid: format!("echo-{label}"),
                description: "This adapter verifies the Hub route without external dependencies."
                    .to_string(),
                published_at: Some(Utc::now()),
            }],
        })
    }
}

struct GithubReleases;

#[derive(Debug, Deserialize)]
struct GithubRelease {
    html_url: String,
    tag_name: String,
    name: Option<String>,
    body: Option<String>,
    published_at: Option<DateTime<Utc>>,
}

#[async_trait]
impl HubAdapter for GithubReleases {
    async fn generate(
        &self,
        _config: &Config,
        http: &reqwest::Client,
        path: &str,
    ) -> AppResult<HubFeed> {
        let parts = path.trim_matches('/').split('/').collect::<Vec<_>>();
        if parts.len() != 2 || parts.iter().any(|part| part.is_empty()) {
            return Err(AppError::BadRequest(
                "GitHub releases 路径必须是 owner/repo".to_string(),
            ));
        }
        let owner = parts[0];
        let repo = parts[1];
        let url = format!("https://api.github.com/repos/{owner}/{repo}/releases?per_page=20");
        let releases = http
            .get(&url)
            .header(reqwest::header::USER_AGENT, "infofetcher/0.1")
            .send()
            .await?
            .error_for_status()?
            .json::<Vec<GithubRelease>>()
            .await?;
        Ok(HubFeed {
            title: format!("{owner}/{repo} releases"),
            link: format!("https://github.com/{owner}/{repo}/releases"),
            description: format!("GitHub releases for {owner}/{repo}"),
            items: releases
                .into_iter()
                .map(|release| HubItem {
                    title: release.name.unwrap_or_else(|| release.tag_name.clone()),
                    link: release.html_url,
                    guid: release.tag_name,
                    description: release.body.unwrap_or_default(),
                    published_at: release.published_at,
                })
                .collect(),
        })
    }
}

struct HackerNewsTop;

#[derive(Debug, Deserialize)]
struct HnItem {
    id: i64,
    title: Option<String>,
    url: Option<String>,
    text: Option<String>,
    time: Option<i64>,
}

#[async_trait]
impl HubAdapter for HackerNewsTop {
    async fn generate(
        &self,
        _config: &Config,
        http: &reqwest::Client,
        _path: &str,
    ) -> AppResult<HubFeed> {
        let ids = http
            .get("https://hacker-news.firebaseio.com/v0/topstories.json")
            .send()
            .await?
            .error_for_status()?
            .json::<Vec<i64>>()
            .await?;
        let mut items = Vec::new();
        for id in ids.into_iter().take(20) {
            let item = http
                .get(format!(
                    "https://hacker-news.firebaseio.com/v0/item/{id}.json"
                ))
                .send()
                .await?
                .error_for_status()?
                .json::<HnItem>()
                .await?;
            let link = item
                .url
                .unwrap_or_else(|| format!("https://news.ycombinator.com/item?id={}", item.id));
            let published_at = item
                .time
                .and_then(|ts| DateTime::<Utc>::from_timestamp(ts, 0));
            items.push(HubItem {
                title: item.title.unwrap_or_else(|| format!("HN item {id}")),
                link,
                guid: format!("hn-{id}"),
                description: item.text.unwrap_or_default(),
                published_at,
            });
        }
        Ok(HubFeed {
            title: "Hacker News Top Stories".to_string(),
            link: "https://news.ycombinator.com/".to_string(),
            description: "Top stories from Hacker News".to_string(),
            items,
        })
    }
}

struct OpenCli;

#[async_trait]
impl HubAdapter for OpenCli {
    async fn generate(
        &self,
        config: &Config,
        _http: &reqwest::Client,
        path: &str,
    ) -> AppResult<HubFeed> {
        let name = path.trim_matches('/');
        if name.is_empty() || name.contains('/') {
            return Err(AppError::BadRequest(
                "OpenCLI 路径必须是 OPENCLI_FEEDS_JSON 中配置的 feed 名称".to_string(),
            ));
        }
        let feed = config.opencli_feed(name).ok_or(AppError::NotFound)?;
        let mut command = Command::new(&config.opencli_bin);
        command
            .args(&feed.args)
            .args(["--format", "json"])
            .kill_on_drop(true);
        let output = time::timeout(config.opencli_timeout, command.output())
            .await
            .map_err(|_| AppError::Upstream(format!("OpenCLI 执行超时: {name}")))?
            .map_err(|err| AppError::Upstream(format!("OpenCLI 启动失败: {err}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AppError::Upstream(format!(
                "OpenCLI 执行失败: {}",
                clip(&stderr, 300)
            )));
        }

        let value = serde_json::from_slice::<Value>(&output.stdout)
            .map_err(|err| AppError::Upstream(format!("OpenCLI 输出不是 JSON: {err}")))?;
        let link = feed
            .link
            .clone()
            .unwrap_or_else(|| "https://opencli.info/".to_string());
        Ok(HubFeed {
            title: feed
                .title
                .clone()
                .unwrap_or_else(|| format!("OpenCLI {name}")),
            link: link.clone(),
            description: feed
                .description
                .clone()
                .unwrap_or_else(|| "OpenCLI generated feed".to_string()),
            items: opencli_items_from_json(name, &link, &value)?,
        })
    }
}

struct ActivityPub;

#[async_trait]
impl HubAdapter for ActivityPub {
    async fn generate(
        &self,
        config: &Config,
        http: &reqwest::Client,
        path: &str,
    ) -> AppResult<HubFeed> {
        let (host, username) = parse_activitypub_path(path)?;
        let webfinger_url = format!(
            "https://{host}/.well-known/webfinger?resource={}",
            urlencoding::encode(&format!("acct:{username}@{host}"))
        );
        let webfinger = get_json(config, http, &webfinger_url, "application/jrd+json").await?;
        let actor_url = webfinger
            .get("links")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|link| {
                link.get("rel").and_then(Value::as_str) == Some("self")
                    && link
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|value| {
                            value.contains("activity+json") || value.contains("ld+json")
                        })
            })
            .and_then(|link| link.get("href"))
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::Upstream("WebFinger 未返回 ActivityPub actor".to_string()))?;
        let actor = get_json(config, http, actor_url, "application/activity+json").await?;
        let outbox_url = actor
            .get("outbox")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::Upstream("ActivityPub actor 缺少 outbox".to_string()))?;
        let mut outbox = get_json(config, http, outbox_url, "application/activity+json").await?;
        if activitypub_items(&outbox).is_none()
            && let Some(first_url) = outbox.get("first").and_then(Value::as_str)
        {
            outbox = get_json(config, http, first_url, "application/activity+json").await?;
        }
        let rows = activitypub_items(&outbox)
            .ok_or_else(|| AppError::Upstream("ActivityPub outbox 没有可读条目".to_string()))?;
        let actor_name = actor
            .get("name")
            .or_else(|| actor.get("preferredUsername"))
            .and_then(Value::as_str)
            .unwrap_or(username);
        let actor_link = actor
            .get("url")
            .and_then(activitypub_url)
            .unwrap_or_else(|| actor_url.to_string());
        let items = rows
            .iter()
            .filter_map(activitypub_item)
            .take(50)
            .collect::<Vec<_>>();
        Ok(HubFeed {
            title: format!("{actor_name} · ActivityPub"),
            link: actor_link,
            description: format!("ActivityPub public outbox for @{username}@{host}"),
            items,
        })
    }
}

struct RssHub;

#[async_trait]
impl HubAdapter for RssHub {
    async fn generate(
        &self,
        config: &Config,
        http: &reqwest::Client,
        path: &str,
    ) -> AppResult<HubFeed> {
        let base = config
            .rsshub_base_url
            .as_deref()
            .ok_or_else(|| AppError::BadRequest("RSSHUB_BASE_URL 未配置".to_string()))?;
        let path = path.trim_matches('/');
        if path.is_empty() {
            return Err(AppError::BadRequest("RSSHub 路径不能为空".to_string()));
        }
        let url = format!("{}/{}", base.trim_end_matches('/'), path);
        let bytes = get_bytes(
            config,
            http,
            &url,
            "application/rss+xml, application/atom+xml",
            true,
        )
        .await?;
        let parsed = parse_feed_bytes(&bytes, &url)?;
        Ok(HubFeed {
            title: parsed.title,
            link: parsed.site_url.unwrap_or_else(|| url.clone()),
            description: format!("RSSHub route /{path}"),
            items: parsed
                .items
                .into_iter()
                .map(|item| HubItem {
                    title: item.title,
                    link: item.url,
                    guid: item.guid,
                    description: item.content,
                    published_at: item.published_at,
                })
                .collect(),
        })
    }
}

struct WebWatch;

#[async_trait]
impl HubAdapter for WebWatch {
    async fn generate(
        &self,
        config: &Config,
        http: &reqwest::Client,
        path: &str,
    ) -> AppResult<HubFeed> {
        let name = path.trim_matches('/');
        if name.is_empty() || name.contains('/') {
            return Err(AppError::BadRequest(
                "Web Watch 路径必须是 WEB_WATCHES_JSON 中的名称".to_string(),
            ));
        }
        let watch = config.web_watch(name).ok_or(AppError::NotFound)?;
        let bytes = get_bytes(
            config,
            http,
            &watch.url,
            "text/html",
            config.allow_private_fetch,
        )
        .await?;
        let html = String::from_utf8_lossy(&bytes);
        let document = Html::parse_document(&html);
        let selector_text = watch.selector.as_deref().unwrap_or("body");
        let selector = Selector::parse(selector_text)
            .map_err(|_| AppError::BadRequest("Web Watch selector 无效".to_string()))?;
        let text = document
            .select(&selector)
            .flat_map(|element| element.text())
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if text.is_empty() {
            return Err(AppError::Upstream(
                "Web Watch selector 没有匹配内容".to_string(),
            ));
        }
        let title = watch.title.clone().unwrap_or_else(|| name.to_string());
        let guid = format!("web-watch:{name}:{}", stable_text_hash(&text));
        Ok(HubFeed {
            title: title.clone(),
            link: watch.url.clone(),
            description: watch
                .description
                .clone()
                .unwrap_or_else(|| format!("网页变化监控：{}", watch.url)),
            items: vec![HubItem {
                title: format!("{title} 内容更新"),
                link: watch.url.clone(),
                guid,
                description: clip(&text, 2_000),
                published_at: None,
            }],
        })
    }
}

async fn get_json(
    config: &Config,
    http: &reqwest::Client,
    url: &str,
    accept: &str,
) -> AppResult<Value> {
    let bytes = get_bytes(config, http, url, accept, config.allow_private_fetch).await?;
    serde_json::from_slice(&bytes)
        .map_err(|err| AppError::Upstream(format!("JSON 解析失败: {err}")))
}

async fn get_bytes(
    config: &Config,
    http: &reqwest::Client,
    url: &str,
    accept: &str,
    allow_private: bool,
) -> AppResult<Vec<u8>> {
    validate_http_url(url, allow_private).await?;
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::USER_AGENT,
        reqwest::header::HeaderValue::from_static("infofetcher/0.1"),
    );
    if let Ok(value) = reqwest::header::HeaderValue::from_str(accept) {
        headers.insert(reqwest::header::ACCEPT, value);
    }
    let response = get_with_safe_redirects(http, url, &headers, allow_private).await?;
    if !response.status().is_success() {
        return Err(AppError::Upstream(format!(
            "上游请求失败: {}",
            response.status()
        )));
    }
    read_response_limited(response, config.feed_max_bytes).await
}

fn parse_activitypub_path(path: &str) -> AppResult<(&str, &str)> {
    let parts = path.trim_matches('/').split('/').collect::<Vec<_>>();
    if parts.len() != 2 {
        return Err(AppError::BadRequest(
            "ActivityPub 路径必须是 host/username".to_string(),
        ));
    }
    let host = parts[0];
    let username = parts[1].trim_start_matches('@');
    if host.is_empty()
        || username.is_empty()
        || !host
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-'))
        || !username
            .chars()
            .all(|character| character.is_alphanumeric() || matches!(character, '_' | '-' | '.'))
    {
        return Err(AppError::BadRequest("ActivityPub actor 无效".to_string()));
    }
    Ok((host, username))
}

fn normalize_activitypub_source(value: &str) -> AppResult<String> {
    let value = value.trim().trim_matches('/');
    let normalized = if value.contains('/') {
        value.to_string()
    } else {
        let value = value.trim_start_matches('@');
        let (username, host) = value
            .rsplit_once('@')
            .ok_or_else(|| AppError::BadRequest("ActivityPub 账号必须是 @user@host".to_string()))?;
        format!("{host}/{username}")
    };
    let (host, username) = parse_activitypub_path(&normalized)?;
    Ok(format!("{}/{username}", host.to_ascii_lowercase()))
}

fn activitypub_items(value: &Value) -> Option<&Vec<Value>> {
    value
        .get("orderedItems")
        .or_else(|| value.get("items"))
        .and_then(Value::as_array)
}

fn activitypub_item(value: &Value) -> Option<HubItem> {
    let object = value.get("object").unwrap_or(value);
    if object.get("type").and_then(Value::as_str) == Some("Tombstone") {
        return None;
    }
    let content = object
        .get("content")
        .or_else(|| object.get("summary"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let clean_content = Html::parse_fragment(content)
        .root_element()
        .text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .or_else(|| value.get("id").and_then(Value::as_str))?;
    let link = object
        .get("url")
        .and_then(activitypub_url)
        .unwrap_or_else(|| id.to_string());
    let title = object
        .get("name")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| clip(&clean_content, 120));
    if title.is_empty() {
        return None;
    }
    let published_at = object
        .get("published")
        .or_else(|| value.get("published"))
        .and_then(Value::as_str)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc));
    Some(HubItem {
        title,
        link,
        guid: id.to_string(),
        description: clean_content,
        published_at,
    })
}

fn activitypub_url(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Object(object) => object
            .get("href")
            .or_else(|| object.get("url"))
            .and_then(Value::as_str)
            .map(ToString::to_string),
        Value::Array(values) => values.iter().find_map(activitypub_url),
        _ => None,
    }
}

fn stable_text_hash(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    hex::encode(&hasher.finalize()[..16])
}

fn opencli_items_from_json(name: &str, feed_link: &str, value: &Value) -> AppResult<Vec<HubItem>> {
    let rows = opencli_rows(value).ok_or_else(|| {
        AppError::Upstream(
            "OpenCLI JSON 必须是数组，或包含 items/data/results/rows 数组".to_string(),
        )
    })?;
    Ok(rows
        .into_iter()
        .take(200)
        .enumerate()
        .map(|(index, row)| opencli_item_from_json(name, feed_link, index, row))
        .collect())
}

fn opencli_rows(value: &Value) -> Option<Vec<&Value>> {
    match value {
        Value::Array(rows) => Some(rows.iter().collect()),
        Value::Object(object) => ["items", "data", "results", "rows"]
            .iter()
            .find_map(|field| object.get(*field)?.as_array())
            .map(|rows| rows.iter().collect()),
        _ => None,
    }
}

fn opencli_item_from_json(name: &str, feed_link: &str, index: usize, value: &Value) -> HubItem {
    let title = first_string_field(
        value,
        &[
            "title",
            "name",
            "text",
            "content",
            "description",
            "summary",
            "topic",
            "word",
            "label",
        ],
    )
    .map(|title| clip(&title, 120))
    .unwrap_or_else(|| format!("OpenCLI item {}", index + 1));
    let link = first_string_field(
        value,
        &["url", "link", "href", "html_url", "web_url", "uri"],
    )
    .and_then(|link| normalize_opencli_link(&link))
    .unwrap_or_else(|| unique_opencli_fallback_link(feed_link, value));
    let guid = first_string_field(value, &["id", "guid", "uuid", "url", "link", "href", "uri"])
        .unwrap_or_else(|| format!("opencli:{name}:{}", stable_opencli_item_hash(value)));
    let description = first_string_field(
        value,
        &["description", "content", "text", "summary", "body", "desc"],
    )
    .unwrap_or_else(|| value_to_description(value));
    let published_at = first_datetime_field(
        value,
        &[
            "published_at",
            "publishedAt",
            "created_at",
            "createdAt",
            "updated_at",
            "updatedAt",
            "published",
            "date",
            "time",
            "timestamp",
        ],
    );

    HubItem {
        title,
        link,
        guid,
        description,
        published_at,
    }
}

fn normalize_opencli_link(value: &str) -> Option<String> {
    if value.starts_with("http://") || value.starts_with("https://") {
        return Some(value.to_string());
    }
    let at_uri = value.strip_prefix("at://")?;
    let mut parts = at_uri.split('/');
    let repository = parts.next()?;
    let collection = parts.next()?;
    let record_key = parts.next()?;
    if repository.is_empty()
        || collection != "app.bsky.feed.post"
        || record_key.is_empty()
        || parts.next().is_some()
    {
        return None;
    }
    Some(format!(
        "https://bsky.app/profile/{}/post/{}",
        urlencoding::encode(repository),
        urlencoding::encode(record_key)
    ))
}

fn unique_opencli_fallback_link(feed_link: &str, value: &Value) -> String {
    if let Ok(mut url) = reqwest::Url::parse(feed_link)
        && matches!(url.scheme(), "http" | "https")
    {
        url.query_pairs_mut()
            .append_pair("opencli_item", &stable_opencli_item_hash(value));
        return url.to_string();
    }
    feed_link.to_string()
}

fn first_string_field(value: &Value, fields: &[&str]) -> Option<String> {
    let object = value.as_object()?;
    fields
        .iter()
        .filter_map(|field| object.get(*field).and_then(value_to_string))
        .find(|value| !value.trim().is_empty())
}

fn value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.trim().to_string()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn first_datetime_field(value: &Value, fields: &[&str]) -> Option<DateTime<Utc>> {
    let object = value.as_object()?;
    fields
        .iter()
        .find_map(|field| object.get(*field).and_then(parse_datetime_value))
}

fn parse_datetime_value(value: &Value) -> Option<DateTime<Utc>> {
    match value {
        Value::String(text) => DateTime::parse_from_rfc3339(text)
            .or_else(|_| DateTime::parse_from_rfc2822(text))
            .or_else(|_| DateTime::parse_from_str(text, "%a %b %d %H:%M:%S %z %Y"))
            .map(|date| date.with_timezone(&Utc))
            .ok()
            .or_else(|| {
                NaiveDate::parse_from_str(text, "%Y-%m-%d")
                    .ok()?
                    .and_hms_opt(0, 0, 0)
                    .map(|date| date.and_utc())
            })
            .or_else(|| text.parse::<i64>().ok().and_then(timestamp_to_datetime)),
        Value::Number(number) => number.as_i64().and_then(timestamp_to_datetime),
        _ => None,
    }
}

fn timestamp_to_datetime(timestamp: i64) -> Option<DateTime<Utc>> {
    let seconds = if timestamp > 1_000_000_000_000 {
        timestamp / 1000
    } else {
        timestamp
    };
    DateTime::<Utc>::from_timestamp(seconds, 0)
}

fn value_to_description(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        _ => serde_json::to_string_pretty(value).unwrap_or_default(),
    }
}

fn stable_json_hash(value: &Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(value).unwrap_or_default());
    hex::encode(&hasher.finalize()[..16])
}

fn stable_opencli_item_hash(value: &Value) -> String {
    let Some(object) = value.as_object() else {
        return stable_json_hash(value);
    };
    let identity = [
        "title",
        "name",
        "text",
        "content",
        "description",
        "summary",
        "topic",
        "word",
        "author",
        "user",
        "handle",
        "published_at",
        "publishedAt",
        "created_at",
        "createdAt",
        "published",
        "date",
    ]
    .iter()
    .filter_map(|field| {
        object
            .get(*field)
            .and_then(value_to_string)
            .map(|value| format!("{field}={value}"))
    })
    .collect::<Vec<_>>();
    if identity.is_empty() {
        stable_json_hash(value)
    } else {
        stable_text_hash(&identity.join("\u{1f}"))
    }
}

fn clip(value: &str, limit: usize) -> String {
    let mut clipped = value.trim().chars().take(limit).collect::<String>();
    if value.trim().chars().count() > limit {
        clipped.push_str("...");
    }
    clipped
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, time::Duration};

    use super::*;
    use crate::{config::OpenCliFeedConfig, feeds::hash_url};

    #[test]
    fn renders_valid_rss_xml() {
        let xml = render_rss(HubFeed {
            title: "Test".to_string(),
            link: "https://example.com".to_string(),
            description: "Desc".to_string(),
            items: vec![HubItem {
                title: "Item".to_string(),
                link: "https://example.com/1".to_string(),
                guid: "1".to_string(),
                description: "Body".to_string(),
                published_at: None,
            }],
        });
        assert!(xml.contains("<rss"));
        assert!(xml.contains("<title>Test</title>"));
        assert!(xml.contains("<title>Item</title>"));
    }

    #[test]
    fn maps_opencli_json_array_to_feed_items() {
        let value = serde_json::json!([
            {
                "id": "note-1",
                "title": "第一条",
                "url": "https://example.com/1",
                "description": "正文",
                "created_at": "2026-07-09T12:00:00Z"
            }
        ]);

        let items = opencli_items_from_json("xhs", "https://example.com", &value).unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "第一条");
        assert_eq!(items[0].link, "https://example.com/1");
        assert_eq!(items[0].guid, "note-1");
        assert_eq!(items[0].description, "正文");
        assert_eq!(
            items[0].published_at.unwrap().to_rfc3339(),
            "2026-07-09T12:00:00+00:00"
        );
    }

    #[test]
    fn parses_opencli_platform_datetime_formats() {
        let twitter = opencli_item_from_json(
            "twitter",
            "https://x.com/dotey",
            0,
            &serde_json::json!({
                "text": "Twitter post",
                "created_at": "Thu Mar 26 10:30:00 +0000 2026"
            }),
        );
        let bilibili = opencli_item_from_json(
            "bilibili",
            "https://space.bilibili.com/1567748478",
            0,
            &serde_json::json!({
                "title": "Bilibili video",
                "date": "2026-04-01"
            }),
        );

        assert_eq!(
            twitter.published_at.unwrap().to_rfc3339(),
            "2026-03-26T10:30:00+00:00"
        );
        assert_eq!(
            bilibili.published_at.unwrap().to_rfc3339(),
            "2026-04-01T00:00:00+00:00"
        );
    }

    #[test]
    fn maps_social_fields_and_keeps_fallback_links_unique() {
        let value = serde_json::json!({
            "items": [
                {"topic": "Bluesky 热门", "rank": 1},
                {"word": "微博热搜", "rank": 2}
            ]
        });

        let items =
            opencli_items_from_json("social", "https://example.com/social", &value).unwrap();

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "Bluesky 热门");
        assert_eq!(items[1].title, "微博热搜");
        assert_ne!(items[0].link, items[1].link);
        assert_ne!(hash_url(&items[0].link), hash_url(&items[1].link));
        assert!(
            items[0]
                .link
                .starts_with("https://example.com/social?opencli_item=")
        );
    }

    #[test]
    fn keeps_social_identity_stable_when_engagement_changes() {
        let first = serde_json::json!([{"text": "Same post", "likes": 10, "rank": 1}]);
        let second = serde_json::json!([{"text": "Same post", "likes": 11, "rank": 2}]);

        let first_item = opencli_items_from_json("social", "https://example.com", &first)
            .unwrap()
            .remove(0);
        let second_item = opencli_items_from_json("social", "https://example.com", &second)
            .unwrap()
            .remove(0);

        assert_eq!(first_item.guid, second_item.guid);
        assert_eq!(hash_url(&first_item.link), hash_url(&second_item.link));
    }

    #[test]
    fn normalizes_supported_social_source_paths() {
        let config = test_config("opencli", Duration::from_secs(1));

        assert_eq!(
            normalize_social_source(&config, "ActivityPub", "@Mastodon@mastodon.social").unwrap(),
            (
                "activitypub".to_string(),
                "mastodon.social/Mastodon".to_string()
            )
        );
        assert_eq!(
            normalize_social_source(&config, "opencli", "bluesky").unwrap(),
            ("opencli".to_string(), "bluesky".to_string())
        );
        assert_eq!(
            normalize_social_source(&config, "rsshub", "/bilibili/popular/all/1").unwrap(),
            ("rsshub".to_string(), "bilibili/popular/all/1".to_string())
        );
        assert!(normalize_social_source(&config, "opencli", "../../bin/sh").is_err());
        assert!(normalize_social_source(&config, "rsshub", "https://evil.test/feed").is_err());
    }

    #[test]
    fn parses_activitypub_note_and_ignores_tombstone() {
        let item = activitypub_item(&serde_json::json!({
            "type": "Create",
            "object": {
                "id": "https://mastodon.social/users/example/statuses/1",
                "type": "Note",
                "content": "<p>Hello <strong>Fediverse</strong></p>",
                "url": "https://mastodon.social/@example/1",
                "published": "2026-07-10T00:00:00Z"
            }
        }))
        .unwrap();
        assert_eq!(item.title, "Hello Fediverse");
        assert_eq!(item.description, "Hello Fediverse");
        assert_eq!(item.link, "https://mastodon.social/@example/1");
        assert!(
            activitypub_item(&serde_json::json!({
                "id": "https://example.com/deleted/1",
                "type": "Tombstone"
            }))
            .is_none()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn executes_allowlisted_opencli_with_fixed_json_format() {
        use std::{fs, os::unix::fs::PermissionsExt};

        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fake-opencli");
        fs::write(
            &executable,
            r#"#!/bin/sh
[ "$#" -eq 7 ] || exit 20
[ "$1" = "bluesky" ] || exit 21
[ "$2" = "user" ] || exit 22
[ "$3" = "bsky.app" ] || exit 23
[ "$4" = "--limit" ] || exit 24
[ "$5" = "1" ] || exit 25
[ "$6" = "--format" ] || exit 26
[ "$7" = "json" ] || exit 27
printf '%s' '[{"uri":"at://did:plc:test/app.bsky.feed.post/record-1","text":"Bluesky post"}]'
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).unwrap();

        let mut config = test_config(executable.to_str().unwrap(), Duration::from_secs(5));
        config.opencli_feeds.insert(
            "bluesky".to_string(),
            OpenCliFeedConfig {
                title: Some("Bluesky".to_string()),
                link: Some("https://bsky.app/profile/bsky.app".to_string()),
                description: None,
                args: vec![
                    "bluesky".to_string(),
                    "user".to_string(),
                    "bsky.app".to_string(),
                    "--limit".to_string(),
                    "1".to_string(),
                ],
            },
        );

        let feed = OpenCli
            .generate(&config, &reqwest::Client::new(), "bluesky")
            .await
            .unwrap();

        assert_eq!(feed.items.len(), 1);
        assert_eq!(feed.items[0].title, "Bluesky post");
        assert_eq!(
            feed.items[0].link,
            "https://bsky.app/profile/did%3Aplc%3Atest/post/record-1"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn times_out_and_drops_slow_opencli_process() {
        use std::{fs, os::unix::fs::PermissionsExt, time::Instant};

        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("slow-opencli");
        fs::write(&executable, "#!/bin/sh\nsleep 2\n").unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).unwrap();
        let config = test_config(executable.to_str().unwrap(), Duration::from_millis(25));

        let started = Instant::now();
        let error = OpenCli
            .generate(&config, &reqwest::Client::new(), "bluesky")
            .await
            .unwrap_err();

        assert!(error.to_string().contains("执行超时"));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    fn test_config(opencli_bin: &str, opencli_timeout: Duration) -> Config {
        let mut opencli_feeds = HashMap::new();
        opencli_feeds.insert(
            "bluesky".to_string(),
            OpenCliFeedConfig {
                title: Some("Bluesky".to_string()),
                link: Some("https://bsky.app".to_string()),
                description: None,
                args: vec!["bluesky".to_string(), "trending".to_string()],
            },
        );
        Config {
            database_url: String::new(),
            app_bind: "127.0.0.1:8080".parse().unwrap(),
            app_public_url: "https://reader.example.com".to_string(),
            admin_username: "admin".to_string(),
            auth_token: "secret".to_string(),
            cookie_secure: false,
            session_days: 14,
            content_retention_days: 7,
            index_dir: ".".into(),
            web_dist_dir: ".".into(),
            fetch_concurrency: 1,
            fetch_interval_minutes: 30,
            feed_max_bytes: 1024 * 1024,
            article_max_bytes: 1024 * 1024,
            allow_private_fetch: false,
            llm_base_url: String::new(),
            llm_api_key: String::new(),
            llm_model: String::new(),
            llm_timeout: Duration::from_secs(1),
            llm_max_concurrency: 1,
            llm_max_output_tokens: 0,
            llm_context_budget_chars: 64_000,
            event_agent_enabled: false,
            event_agent_interval: Duration::from_secs(30),
            event_agent_lookback_hours: 24,
            event_agent_max_pending: 12,
            event_agent_update_debounce: Duration::from_secs(30 * 60),
            translation_base_url: "https://api.deepseek.com".to_string(),
            translation_api_key: String::new(),
            translation_model: "deepseek-v4-flash".to_string(),
            translation_timeout: Duration::from_secs(1),
            translation_max_concurrency: 1,
            translation_max_input_chars: 60_000,
            translation_chunk_chars: 6_000,
            translation_max_output_tokens: 8_192,
            telegram_api_base: "https://api.telegram.org".to_string(),
            telegram_bot_token: String::new(),
            telegram_chat_id: String::new(),
            telegram_timeout: Duration::from_secs(1),
            opencli_bin: opencli_bin.to_string(),
            opencli_timeout,
            opencli_max_concurrency: 1,
            opencli_access_token: String::new(),
            rsshub_base_url: Some("https://rsshub.example.com".to_string()),
            websub_enabled: false,
            websub_lease_seconds: 3600,
            browser_fetch_enabled: false,
            browser_fetch_bin: "chromium".to_string(),
            browser_fetch_user_data_dir: None,
            browser_fetch_timeout: Duration::from_secs(1),
            browser_fetch_wait_ms: 1,
            article_fetch_cookies: Default::default(),
            opencli_feeds,
            web_watches: Default::default(),
        }
    }
}
