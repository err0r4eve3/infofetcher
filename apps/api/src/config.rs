use std::{collections::HashMap, env, net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use serde::Deserialize;

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub app_bind: SocketAddr,
    pub app_public_url: String,
    pub admin_username: String,
    pub auth_token: String,
    pub cookie_secure: bool,
    pub session_days: i64,
    pub content_retention_days: i64,
    pub index_dir: PathBuf,
    pub web_dist_dir: PathBuf,
    pub fetch_concurrency: usize,
    pub fetch_interval_minutes: i32,
    pub feed_max_bytes: usize,
    pub article_max_bytes: usize,
    pub allow_private_fetch: bool,
    pub osint_searxng_url: Option<String>,
    /// 只允许访问 `/api/v2/intel/*` 的只读服务凭据，供 TradingAgents 等外部服务使用。
    pub intel_api_token: Option<String>,
    /// OpenAI 兼容的向量服务（本机 llama.cpp）。未配置时不做向量召回。
    pub embedding_base_url: Option<String>,
    pub embedding_model: String,
    pub embedding_timeout: Duration,
    pub llm_base_url: String,
    pub llm_api_key: String,
    pub llm_model: String,
    pub llm_timeout: Duration,
    pub llm_max_concurrency: usize,
    pub llm_max_output_tokens: u32,
    pub llm_context_budget_chars: usize,
    pub event_agent_enabled: bool,
    pub event_agent_interval: Duration,
    pub event_agent_lookback_hours: i64,
    pub event_agent_max_pending: usize,
    pub event_agent_update_debounce: Duration,
    pub translation_base_url: String,
    pub translation_api_key: String,
    pub translation_model: String,
    pub translation_timeout: Duration,
    pub translation_max_concurrency: usize,
    pub translation_max_input_chars: usize,
    pub translation_chunk_chars: usize,
    pub translation_max_output_tokens: u32,
    pub telegram_api_base: String,
    pub telegram_bot_token: String,
    pub telegram_chat_id: String,
    pub telegram_timeout: Duration,
    pub opencli_bin: String,
    pub opencli_timeout: Duration,
    pub opencli_max_concurrency: usize,
    pub opencli_access_token: String,
    pub rsshub_base_url: Option<String>,
    pub websub_enabled: bool,
    pub websub_lease_seconds: i64,
    pub browser_fetch_enabled: bool,
    pub browser_fetch_bin: String,
    pub browser_fetch_user_data_dir: Option<PathBuf>,
    pub browser_fetch_timeout: Duration,
    pub browser_fetch_wait_ms: u64,
    pub(crate) article_fetch_cookies: HashMap<String, String>,
    pub(crate) opencli_feeds: HashMap<String, OpenCliFeedConfig>,
    pub(crate) web_watches: HashMap<String, WebWatchConfig>,
    pub editorial: EditorialConfig,
}

/// 编辑层配置：LLM 评审、栏目配额、日报分发和 MCP。见 docs/EDITORIAL_LAYER.md。
#[derive(Debug, Clone)]
pub struct EditorialConfig {
    pub enabled: bool,
    pub interval: Duration,
    pub daily_event_limit: i64,
    /// Profile id 到进入简报的最低分（0–10）。
    pub thresholds: HashMap<String, f32>,
    /// 栏目到简报条数上限比例。
    pub section_shares: HashMap<String, f32>,
    pub edition_hour: u32,
    pub edition_utc_offset_hours: i32,
    pub brief_feed_token: Option<String>,
    pub brief_webhook_url: Option<String>,
    pub brief_webhook_format: String,
    pub brief_telegram: bool,
    pub mcp_api_token: Option<String>,
}

impl Default for EditorialConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval: Duration::from_secs(600),
            daily_event_limit: 600,
            thresholds: parse_number_map(DEFAULT_EDITORIAL_THRESHOLDS),
            section_shares: parse_number_map(DEFAULT_BRIEF_SECTION_SHARES),
            edition_hour: 8,
            edition_utc_offset_hours: 8,
            brief_feed_token: None,
            brief_webhook_url: None,
            brief_webhook_format: "generic".to_string(),
            brief_telegram: false,
            mcp_api_token: None,
        }
    }
}

impl EditorialConfig {
    pub fn threshold(&self, profile: &str) -> f32 {
        self.thresholds.get(profile).copied().unwrap_or(5.0)
    }

    fn from_env(auth_token: &str, intel_api_token: Option<&str>) -> anyhow::Result<Self> {
        let brief_feed_token = env_optional_string("BRIEF_FEED_TOKEN");
        let mcp_api_token = env_optional_string("MCP_API_TOKEN");
        for (name, token) in [
            ("BRIEF_FEED_TOKEN", brief_feed_token.as_deref()),
            ("MCP_API_TOKEN", mcp_api_token.as_deref()),
        ] {
            if let Some(token) = token {
                validate_service_token(name, token, auth_token)?;
                if Some(token) == intel_api_token {
                    anyhow::bail!("{name} 不能与 INTEL_API_TOKEN 相同");
                }
            }
        }
        if brief_feed_token.is_some() && brief_feed_token == mcp_api_token {
            anyhow::bail!("BRIEF_FEED_TOKEN 不能与 MCP_API_TOKEN 相同");
        }
        let brief_webhook_format = env_string("BRIEF_WEBHOOK_FORMAT", "generic");
        if !matches!(brief_webhook_format.as_str(), "generic" | "feishu") {
            anyhow::bail!("BRIEF_WEBHOOK_FORMAT 只支持 generic 或 feishu");
        }
        Ok(Self {
            enabled: env_bool("EDITORIAL_ENABLED", false),
            interval: Duration::from_secs(
                env_u64("EDITORIAL_INTERVAL_SECONDS", 600).clamp(60, 6 * 3_600),
            ),
            daily_event_limit: env_i64("EDITORIAL_DAILY_EVENT_LIMIT", 600).clamp(0, 10_000),
            thresholds: parse_number_map(&env_string(
                "EDITORIAL_THRESHOLDS",
                DEFAULT_EDITORIAL_THRESHOLDS,
            ))
            .into_iter()
            .map(|(key, value)| (key, value.clamp(0.0, 10.0)))
            .collect(),
            section_shares: parse_number_map(&env_string(
                "BRIEF_SECTION_SHARES",
                DEFAULT_BRIEF_SECTION_SHARES,
            ))
            .into_iter()
            .map(|(key, value)| (key, value.clamp(0.0, 1.0)))
            .inspect(|(key, _)| {
                // 栏目细分后 compose 里的旧默认值（world、technology、security）静默失配了一天多。
                if !crate::world::REPORT_SECTIONS.iter().any(|(section, _)| section == key) {
                    tracing::warn!(section = %key, "BRIEF_SECTION_SHARES has an unknown section key");
                }
            })
            .collect(),
            edition_hour: env_u64("BRIEF_EDITION_HOUR", 8).min(23) as u32,
            edition_utc_offset_hours: env_i64("BRIEF_EDITION_UTC_OFFSET_HOURS", 8).clamp(-12, 14)
                as i32,
            brief_feed_token,
            brief_webhook_url: env_optional_string("BRIEF_WEBHOOK_URL"),
            brief_webhook_format,
            brief_telegram: env_bool("BRIEF_TELEGRAM", false),
            mcp_api_token,
        })
    }
}

const DEFAULT_EDITORIAL_THRESHOLDS: &str = "world-news:5,tech-research:5,long-read:5";
const DEFAULT_BRIEF_SECTION_SHARES: &str =
    "conflict:0.25,politics:0.25,society:0.2,disaster:0.15,economy:0.2,business:0.15,ai:0.2,tech:0.2,cyber:0.1,science:0.15,crypto:0.1,culture:0.1,general:0.15";

#[cfg(test)]
mod section_share_tests {
    #[test]
    fn default_section_shares_use_current_section_keys() {
        for (key, _) in super::parse_number_map(super::DEFAULT_BRIEF_SECTION_SHARES) {
            assert!(crate::world::REPORT_SECTIONS.iter().any(|(section, _)| *section == key), "{key}");
        }
        let compose = include_str!("../../../docker-compose.yml");
        assert!(compose.contains(super::DEFAULT_BRIEF_SECTION_SHARES), "compose default must match");
    }
}

/// 解析 `key:number,key:number`；格式错误的项直接忽略。
fn parse_number_map(raw: &str) -> HashMap<String, f32> {
    raw.split(',')
        .filter_map(|pair| {
            let (key, value) = pair.split_once(':')?;
            let key = key.trim();
            let value = value.trim().parse::<f32>().ok()?;
            (!key.is_empty() && value.is_finite()).then(|| (key.to_string(), value))
        })
        .collect()
}

#[derive(Debug, Clone, Deserialize)]
pub struct OpenCliFeedConfig {
    pub title: Option<String>,
    pub link: Option<String>,
    pub description: Option<String>,
    pub args: Vec<String>,
    #[serde(default)]
    pub filter: Option<OpenCliFilterConfig>,
}

/// 社媒条目过滤：先按作者、关键词和语言白名单做本地规则过滤，
/// 配置 `llm_rubric` 时再用 LLM 按标准批量判断保留与否。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OpenCliFilterConfig {
    #[serde(default)]
    pub exclude_authors: Vec<String>,
    #[serde(default)]
    pub exclude_keywords: Vec<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub llm_rubric: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WebWatchConfig {
    pub title: Option<String>,
    pub url: String,
    pub selector: Option<String>,
    pub description: Option<String>,
    /// 设置后按文章列表处理：页面上 href 以此开头的每个链接生成一条，而不是整页变化一条。
    #[serde(default)]
    pub link_prefix: Option<String>,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Arc<Self>> {
        let database_url = env_string(
            "DATABASE_URL",
            "postgres://infofetcher:reader@localhost:5432/infofetcher",
        );
        let app_bind = env_string("APP_BIND", "0.0.0.0:8080").parse()?;
        let app_public_url = env_string("APP_PUBLIC_URL", "http://localhost:8080");
        let admin_username = env_string("ADMIN_USERNAME", "admin");
        let auth_token = env_string("AUTH_TOKEN", "");
        let cookie_secure = env_bool("COOKIE_SECURE", app_public_url.starts_with("https://"));
        let session_days = env_i64("SESSION_DAYS", 14);
        let content_retention_days = env_i64("CONTENT_RETENTION_DAYS", 365).clamp(1, 365);
        let index_dir = PathBuf::from(env_string("INDEX_DIR", "./data/tantivy"));
        let web_dist_dir = PathBuf::from(env_string("WEB_DIST_DIR", "./apps/web/dist"));
        let fetch_concurrency = env_usize("FETCH_CONCURRENCY", 8).max(1);
        let fetch_interval_minutes = env_i32("FETCH_INTERVAL_MINUTES", 60).max(1);
        let feed_max_bytes = env_usize("FEED_MAX_BYTES", 5 * 1024 * 1024).max(64 * 1024);
        let article_max_bytes = env_usize("ARTICLE_MAX_BYTES", 8 * 1024 * 1024).max(64 * 1024);
        let allow_private_fetch = env_bool("ALLOW_PRIVATE_FETCH", false);
        let llm_base_url = env_string("LLM_BASE_URL", "https://cpa1.cat-infra.com/v1");
        let llm_api_key = env_string("LLM_API_KEY", "");
        let llm_model = env_string("LLM_MODEL", "claude-haiku-5-5");
        let llm_timeout = Duration::from_secs(env_u64("LLM_TIMEOUT_SECONDS", 45).max(1));
        let llm_max_concurrency = env_usize("LLM_MAX_CONCURRENCY", 2).clamp(1, 8);
        let llm_max_output_tokens = env_u64("LLM_MAX_OUTPUT_TOKENS", 4_096).min(32_768) as u32;
        let llm_context_budget_chars =
            env_usize("LLM_CONTEXT_BUDGET_CHARS", 64_000).clamp(16_000, 2_000_000);
        let event_agent_enabled = env_bool("EVENT_AGENT_ENABLED", false);
        let event_agent_interval =
            Duration::from_secs(env_u64("EVENT_AGENT_INTERVAL_SECONDS", 30).clamp(10, 3_600));
        let event_agent_lookback_hours =
            env_i64("EVENT_AGENT_LOOKBACK_HOURS", 24).clamp(1, 24 * 30);
        let event_agent_max_pending = env_usize("EVENT_AGENT_MAX_PENDING", 12).clamp(1, 100);
        let event_agent_update_debounce = Duration::from_secs(
            env_u64("EVENT_AGENT_UPDATE_DEBOUNCE_SECONDS", 30 * 60).min(6 * 60 * 60),
        );
        let translation_base_url = env_string("TRANSLATION_BASE_URL", "https://cpa1.cat-infra.com/v1");
        let translation_api_key = env_string("TRANSLATION_API_KEY", "");
        let translation_model = env_string("TRANSLATION_MODEL", "claude-haiku-5-5");
        let translation_timeout =
            Duration::from_secs(env_u64("TRANSLATION_TIMEOUT_SECONDS", 60).max(1));
        let translation_max_concurrency = env_usize("TRANSLATION_MAX_CONCURRENCY", 2).clamp(1, 4);
        let translation_max_input_chars =
            env_usize("TRANSLATION_MAX_INPUT_CHARS", 60_000).clamp(1_000, 500_000);
        let translation_chunk_chars =
            env_usize("TRANSLATION_CHUNK_CHARS", 6_000).clamp(1_000, 12_000);
        let translation_max_output_tokens =
            env_u64("TRANSLATION_MAX_OUTPUT_TOKENS", 8_192).clamp(256, 32_768) as u32;
        let telegram_api_base = env_string("TELEGRAM_API_BASE", "https://api.telegram.org");
        let telegram_bot_token = env_string("TELEGRAM_BOT_TOKEN", "");
        let telegram_chat_id = env_string("TELEGRAM_CHAT_ID", "");
        let telegram_timeout = Duration::from_secs(env_u64("TELEGRAM_TIMEOUT_SECONDS", 20).max(1));
        let opencli_bin = env_string("OPENCLI_BIN", "opencli");
        let opencli_timeout = Duration::from_secs(env_u64("OPENCLI_TIMEOUT_SECONDS", 90).max(1));
        let opencli_max_concurrency = env_usize("OPENCLI_MAX_CONCURRENCY", 1).clamp(1, 8);
        let opencli_access_token = env_string("OPENCLI_ACCESS_TOKEN", "");
        let rsshub_base_url = env_optional_string("RSSHUB_BASE_URL");
        let websub_enabled = env_bool("WEBSUB_ENABLED", false);
        let websub_lease_seconds = env_i64("WEBSUB_LEASE_SECONDS", 7 * 24 * 60 * 60).max(300);
        let browser_fetch_enabled = env_bool("BROWSER_FETCH_ENABLED", false);
        let browser_fetch_bin = env_string("BROWSER_FETCH_BIN", "chromium");
        let browser_fetch_user_data_dir = env_optional_path("BROWSER_FETCH_USER_DATA_DIR");
        let browser_fetch_timeout =
            Duration::from_secs(env_u64("BROWSER_FETCH_TIMEOUT_SECONDS", 45).max(1));
        let browser_fetch_wait_ms = env_u64("BROWSER_FETCH_WAIT_MS", 5_000);
        let article_fetch_cookies = parse_article_fetch_cookies()?;
        let opencli_feeds = parse_opencli_feeds()?;
        let web_watches = parse_web_watches()?;

        validate_auth_token(&auth_token)?;
        let intel_api_token = env_optional_string("INTEL_API_TOKEN");
        if let Some(token) = &intel_api_token {
            validate_intel_api_token(token, &auth_token)?;
        }
        let editorial = EditorialConfig::from_env(&auth_token, intel_api_token.as_deref())?;
        if editorial.enabled && llm_api_key.trim().is_empty() {
            anyhow::bail!("EDITORIAL_ENABLED=true 时需要配置 LLM_API_KEY");
        }
        if event_agent_enabled
            && llm_api_key.trim().is_empty()
            && translation_api_key.trim().is_empty()
        {
            anyhow::bail!(
                "EVENT_AGENT_ENABLED=true 时至少需要配置 LLM_API_KEY 或 TRANSLATION_API_KEY"
            );
        }

        Ok(Arc::new(Self {
            database_url,
            app_bind,
            app_public_url,
            admin_username,
            auth_token,
            cookie_secure,
            session_days,
            content_retention_days,
            index_dir,
            web_dist_dir,
            fetch_concurrency,
            fetch_interval_minutes,
            feed_max_bytes,
            article_max_bytes,
            allow_private_fetch,
            osint_searxng_url: env_optional_string("OSINT_SEARXNG_URL"),
            intel_api_token,
            embedding_base_url: env_optional_string("EMBEDDING_BASE_URL"),
            embedding_model: env_string("EMBEDDING_MODEL", "embeddinggemma-2"),
            embedding_timeout: Duration::from_secs(env_u64("EMBEDDING_TIMEOUT_SECONDS", 30).max(1)),
            llm_base_url,
            llm_api_key,
            llm_model,
            llm_timeout,
            llm_max_concurrency,
            llm_max_output_tokens,
            llm_context_budget_chars,
            event_agent_enabled,
            event_agent_interval,
            event_agent_lookback_hours,
            event_agent_max_pending,
            event_agent_update_debounce,
            translation_base_url,
            translation_api_key,
            translation_model,
            translation_timeout,
            translation_max_concurrency,
            translation_max_input_chars,
            translation_chunk_chars,
            translation_max_output_tokens,
            telegram_api_base,
            telegram_bot_token,
            telegram_chat_id,
            telegram_timeout,
            opencli_bin,
            opencli_timeout,
            opencli_max_concurrency,
            opencli_access_token,
            rsshub_base_url,
            websub_enabled,
            websub_lease_seconds,
            browser_fetch_enabled,
            browser_fetch_bin,
            browser_fetch_user_data_dir,
            browser_fetch_timeout,
            browser_fetch_wait_ms,
            article_fetch_cookies,
            opencli_feeds,
            web_watches,
            editorial,
        }))
    }

    pub fn article_fetch_cookie_for(&self, url: &str) -> Option<&str> {
        let host = reqwest::Url::parse(url)
            .ok()?
            .host_str()?
            .to_ascii_lowercase();
        self.article_fetch_cookies.get(&host).map(String::as_str)
    }

    pub fn opencli_feed(&self, name: &str) -> Option<&OpenCliFeedConfig> {
        self.opencli_feeds.get(name)
    }

    pub fn opencli_feed_entries(&self) -> impl Iterator<Item = (&str, &OpenCliFeedConfig)> {
        self.opencli_feeds
            .iter()
            .map(|(name, feed)| (name.as_str(), feed))
    }

    pub fn rsshub_enabled(&self) -> bool {
        self.rsshub_base_url.is_some()
    }

    pub fn web_watch(&self, name: &str) -> Option<&WebWatchConfig> {
        self.web_watches.get(name)
    }
}

fn validate_auth_token(token: &str) -> anyhow::Result<()> {
    if !(32..=256).contains(&token.len()) || !token.bytes().all(|b| b.is_ascii_graphic()) {
        anyhow::bail!("AUTH_TOKEN 必须为 32–256 个无空白 ASCII 字符，请使用随机生成的 token");
    }
    Ok(())
}

fn validate_intel_api_token(token: &str, auth_token: &str) -> anyhow::Result<()> {
    validate_service_token("INTEL_API_TOKEN", token, auth_token)
}

fn validate_service_token(name: &str, token: &str, auth_token: &str) -> anyhow::Result<()> {
    if !(32..=256).contains(&token.len()) || !token.bytes().all(|b| b.is_ascii_graphic()) {
        anyhow::bail!("{name} 必须为 32–256 个无空白 ASCII 字符，请使用随机生成的 token");
    }
    if token == auth_token {
        anyhow::bail!("{name} 不能与 AUTH_TOKEN 相同");
    }
    Ok(())
}

fn env_string(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_optional_path(key: &str) -> Option<PathBuf> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn env_optional_string(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_article_fetch_cookies() -> anyhow::Result<HashMap<String, String>> {
    let raw = env_string("ARTICLE_FETCH_COOKIES_JSON", "{}");
    let cookies = serde_json::from_str::<HashMap<String, String>>(&raw).map_err(|err| {
        anyhow::anyhow!(
            "ARTICLE_FETCH_COOKIES_JSON 必须是 hostname 到 Cookie header 的 JSON 对象: {err}"
        )
    })?;
    Ok(cookies
        .into_iter()
        .map(|(host, cookie)| (host.trim().to_ascii_lowercase(), cookie))
        .filter(|(host, cookie)| !host.is_empty() && !cookie.trim().is_empty())
        .collect())
}

fn parse_opencli_feeds() -> anyhow::Result<HashMap<String, OpenCliFeedConfig>> {
    parse_opencli_feeds_json(&env_string("OPENCLI_FEEDS_JSON", "{}"))
}

fn parse_opencli_feeds_json(raw: &str) -> anyhow::Result<HashMap<String, OpenCliFeedConfig>> {
    let feeds = serde_json::from_str::<HashMap<String, OpenCliFeedConfig>>(raw).map_err(|err| {
        anyhow::anyhow!("OPENCLI_FEEDS_JSON 必须是 name 到 OpenCLI feed 配置的 JSON 对象: {err}")
    })?;
    feeds
        .into_iter()
        .map(|(name, feed)| {
            let name = name.trim().trim_matches('/').to_string();
            if name.is_empty() || name.contains('/') {
                anyhow::bail!("OPENCLI_FEEDS_JSON 的 name 不能为空，也不能包含 /");
            }
            let args = feed
                .args
                .into_iter()
                .map(|arg| arg.trim().to_string())
                .filter(|arg| !arg.is_empty())
                .collect::<Vec<_>>();
            if args.is_empty() {
                anyhow::bail!("OPENCLI_FEEDS_JSON.{name}.args 不能为空");
            }
            Ok((
                name,
                OpenCliFeedConfig {
                    title: feed.title,
                    link: feed.link,
                    description: feed.description,
                    args,
                    filter: feed.filter.map(normalize_opencli_filter),
                },
            ))
        })
        .collect()
}

fn normalize_opencli_filter(filter: OpenCliFilterConfig) -> OpenCliFilterConfig {
    let normalize = |values: Vec<String>, strip: &[char]| {
        values
            .into_iter()
            .map(|value| value.trim().trim_start_matches(strip).to_lowercase())
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
    };
    OpenCliFilterConfig {
        exclude_authors: normalize(filter.exclude_authors, &['@']),
        exclude_keywords: normalize(filter.exclude_keywords, &[]),
        languages: normalize(filter.languages, &[]),
        llm_rubric: filter
            .llm_rubric
            .map(|rubric| rubric.trim().to_string())
            .filter(|rubric| !rubric.is_empty()),
    }
}

fn parse_web_watches() -> anyhow::Result<HashMap<String, WebWatchConfig>> {
    parse_web_watches_json(&env_string("WEB_WATCHES_JSON", "{}"))
}

fn parse_web_watches_json(raw: &str) -> anyhow::Result<HashMap<String, WebWatchConfig>> {
    let watches = serde_json::from_str::<HashMap<String, WebWatchConfig>>(raw)
        .map_err(|err| anyhow::anyhow!("WEB_WATCHES_JSON 配置无效: {err}"))?;
    watches
        .into_iter()
        .map(|(name, mut watch)| {
            let name = name.trim().trim_matches('/').to_string();
            if name.is_empty() || name.contains('/') {
                anyhow::bail!("WEB_WATCHES_JSON 的 name 不能为空，也不能包含 /");
            }
            if !(watch.url.starts_with("http://") || watch.url.starts_with("https://")) {
                anyhow::bail!("WEB_WATCHES_JSON.{name}.url 必须是 http/https URL");
            }
            watch.url = watch.url.trim().to_string();
            Ok((name, watch))
        })
        .collect()
}

fn env_i64(key: &str, default: i64) -> i64 {
    env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_i32(key: &str, default: i32) -> i32 {
    env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_u64(key: &str, default: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_bool(key: &str, default: bool) -> bool {
    env::var(key)
        .ok()
        .and_then(|value| match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        })
        .unwrap_or(default)
}

fn env_usize(key: &str, default: usize) -> usize {
    env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_opencli_feed_allowlist() {
        let feeds = parse_opencli_feeds_json(
            r#"{"xhs-ai":{"title":"XHS AI","args":["xiaohongshu","search","AI","--limit","20"]}}"#,
        )
        .unwrap();

        let feed = feeds.get("xhs-ai").unwrap();
        assert_eq!(feed.title.as_deref(), Some("XHS AI"));
        assert_eq!(feed.args, ["xiaohongshu", "search", "AI", "--limit", "20"]);
    }

    #[test]
    fn rejects_opencli_feed_without_args() {
        let error = parse_opencli_feeds_json(r#"{"bad":{"args":[]}}"#).unwrap_err();
        assert!(error.to_string().contains("args 不能为空"));
    }

    #[test]
    fn validates_auth_token() {
        assert!(validate_auth_token(&"a".repeat(32)).is_ok());
        for invalid in [String::new(), "a".repeat(31), "a".repeat(257), " ".repeat(32)] {
            assert!(validate_auth_token(&invalid).is_err());
        }
    }

    #[test]
    fn parses_web_watch_allowlist() {
        let watches = parse_web_watches_json(
            r#"{"release":{"url":"https://example.com/releases","selector":"main"}}"#,
        )
        .unwrap();
        assert_eq!(
            watches.get("release").unwrap().selector.as_deref(),
            Some("main")
        );
    }
}
