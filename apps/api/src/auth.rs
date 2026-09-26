use axum::http::{HeaderMap, HeaderValue, header};
use chrono::{Duration, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::{
    config::Config,
    error::{AppError, AppResult},
};

pub const SESSION_COOKIE: &str = "rss_session";

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub username: String,
}

pub async fn create_session(
    pool: &PgPool,
    config: &Config,
    username: &str,
) -> AppResult<(String, HeaderMap)> {
    let token = format!("{}{}", Uuid::new_v4(), Uuid::new_v4());
    let token_hash = hash_token(&token);
    let expires_at = Utc::now() + Duration::days(config.session_days);
    sqlx::query("INSERT INTO sessions (token_hash, username, expires_at) VALUES ($1, $2, $3)")
        .bind(&token_hash)
        .bind(username)
        .bind(expires_at)
        .execute(pool)
        .await?;

    let max_age = config.session_days * 24 * 60 * 60;
    let secure = if config.cookie_secure { "; Secure" } else { "" };
    let cookie = format!(
        "{SESSION_COOKIE}={token}; Path=/; Max-Age={max_age}; HttpOnly; SameSite=Lax{secure}"
    );
    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).map_err(|err| AppError::BadRequest(err.to_string()))?,
    );
    Ok((username.to_string(), headers))
}

pub async fn clear_session(
    pool: &PgPool,
    config: &Config,
    headers: &HeaderMap,
) -> AppResult<HeaderMap> {
    if let Some(token) = read_cookie(headers, SESSION_COOKIE) {
        let token_hash = hash_token(&token);
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(token_hash)
            .execute(pool)
            .await?;
    }
    let mut response_headers = HeaderMap::new();
    let secure = if config.cookie_secure { "; Secure" } else { "" };
    response_headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "rss_session=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax{secure}"
        ))
        .map_err(|err| AppError::BadRequest(err.to_string()))?,
    );
    Ok(response_headers)
}

pub async fn require_session(pool: &PgPool, headers: &HeaderMap) -> AppResult<String> {
    let token = read_cookie(headers, SESSION_COOKIE).ok_or(AppError::Unauthorized)?;
    let token_hash = hash_token(&token);
    let row =
        sqlx::query("SELECT username FROM sessions WHERE token_hash = $1 AND expires_at > now()")
            .bind(token_hash)
            .fetch_optional(pool)
            .await?;
    row.map(|row| row.get::<String, _>("username"))
        .ok_or(AppError::Unauthorized)
}

pub fn verify_token(config: &Config, token: &str) -> bool {
    !token.is_empty() && constant_time_eq(&config.auth_token, token)
}

pub async fn active_username(pool: &PgPool, config: &Config) -> AppResult<String> {
    Ok(sqlx::query_scalar::<_, String>("SELECT username FROM admin_credentials WHERE id = true")
        .fetch_optional(pool)
        .await?
        .unwrap_or_else(|| config.admin_username.clone()))
}

fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    let mut diff = left.len() ^ right.len();
    for index in 0..left.len().max(right.len()) {
        let left_byte = left.get(index).copied().unwrap_or(0);
        let right_byte = right.get(index).copied().unwrap_or(0);
        diff |= usize::from(left_byte ^ right_byte);
    }
    diff == 0
}

fn read_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find_map(|(key, value)| (key == name).then(|| value.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_named_cookie_from_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("a=1; rss_session=abc; b=2"),
        );
        assert_eq!(
            read_cookie(&headers, SESSION_COOKIE),
            Some("abc".to_string())
        );
    }

    #[test]
    fn login_requires_exact_token() {
        let config = Config {
            database_url: String::new(),
            app_bind: "127.0.0.1:8080".parse().unwrap(),
            app_public_url: String::new(),
            admin_username: "admin".to_string(),
            auth_token: "secret".to_string(),
            cookie_secure: false,
            session_days: 14,
            content_retention_days: 7,
            index_dir: ".".into(),
            web_dist_dir: ".".into(),
            fetch_concurrency: 32,
            fetch_interval_minutes: 30,
            feed_max_bytes: 1024 * 1024,
            article_max_bytes: 1024 * 1024,
            allow_private_fetch: true,
            llm_base_url: String::new(),
            llm_api_key: String::new(),
            llm_model: String::new(),
            llm_timeout: std::time::Duration::from_secs(1),
            llm_max_concurrency: 1,
            llm_max_output_tokens: 0,
            llm_context_budget_chars: 64_000,
            event_agent_enabled: false,
            osint_searxng_url: None,
            event_agent_interval: std::time::Duration::from_secs(30),
            event_agent_lookback_hours: 24,
            event_agent_max_pending: 12,
            event_agent_update_debounce: std::time::Duration::from_secs(30 * 60),
            translation_base_url: "https://api.deepseek.com".to_string(),
            translation_api_key: String::new(),
            translation_model: "deepseek-v4-flash".to_string(),
            translation_timeout: std::time::Duration::from_secs(1),
            translation_max_concurrency: 1,
            translation_max_input_chars: 60_000,
            translation_chunk_chars: 6_000,
            translation_max_output_tokens: 8_192,
            telegram_api_base: "https://api.telegram.org".to_string(),
            telegram_bot_token: String::new(),
            telegram_chat_id: String::new(),
            telegram_timeout: std::time::Duration::from_secs(1),
            opencli_bin: "opencli".to_string(),
            opencli_timeout: std::time::Duration::from_secs(1),
            opencli_max_concurrency: 1,
            opencli_access_token: String::new(),
            rsshub_base_url: None,
            websub_enabled: false,
            websub_lease_seconds: 3600,
            browser_fetch_enabled: false,
            browser_fetch_bin: "chromium".to_string(),
            browser_fetch_user_data_dir: None,
            browser_fetch_timeout: std::time::Duration::from_secs(1),
            browser_fetch_wait_ms: 1,
            article_fetch_cookies: Default::default(),
            opencli_feeds: Default::default(),
            web_watches: Default::default(),
        };
        assert!(verify_token(&config, "secret"));
        assert!(!verify_token(&config, "wrong"));
        assert!(!verify_token(&config, ""));
        assert!(!verify_token(&config, "secret "));
    }

    #[test]
    fn constant_time_equality_handles_length_mismatches() {
        assert!(constant_time_eq("same", "same"));
        assert!(!constant_time_eq("same", "same-but-longer"));
        assert!(!constant_time_eq("same", "diff"));
    }
}
