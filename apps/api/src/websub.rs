use axum::http::HeaderMap;
use hmac::{Hmac, Mac};
use sha1::Sha1;
use sha2::Sha256;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    net::validate_http_url,
    state::AppState,
    worker::{self, WorkerCommand},
};

pub async fn maybe_subscribe(state: &AppState, feed_id: Uuid) -> AppResult<bool> {
    if !state.config.websub_enabled {
        return Ok(false);
    }
    let row = sqlx::query(
        "SELECT websub_hub_url, websub_topic_url, websub_secret, websub_lease_expires_at
         FROM feeds WHERE id = $1",
    )
    .bind(feed_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    let Some(hub_url) = row.get::<Option<String>, _>("websub_hub_url") else {
        return Ok(false);
    };
    let Some(topic_url) = row.get::<Option<String>, _>("websub_topic_url") else {
        return Ok(false);
    };
    let lease_expires =
        row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("websub_lease_expires_at");
    if lease_expires
        .is_some_and(|expires| expires > chrono::Utc::now() + chrono::Duration::hours(12))
    {
        return Ok(false);
    }
    validate_http_url(&hub_url, state.config.allow_private_fetch).await?;
    let secret = row
        .get::<Option<String>, _>("websub_secret")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("{}{}", Uuid::new_v4(), Uuid::new_v4()));
    sqlx::query("UPDATE feeds SET websub_secret = $2 WHERE id = $1")
        .bind(feed_id)
        .bind(&secret)
        .execute(&state.pool)
        .await?;
    let callback = format!(
        "{}/api/websub/{}",
        state.config.app_public_url.trim_end_matches('/'),
        feed_id
    );
    let body = format!(
        "hub.mode=subscribe&hub.topic={}&hub.callback={}&hub.verify=async&hub.lease_seconds={}&hub.secret={}",
        urlencoding::encode(&topic_url),
        urlencoding::encode(&callback),
        state.config.websub_lease_seconds,
        urlencoding::encode(&secret)
    );
    let response = state
        .http
        .post(hub_url)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .header(reqwest::header::USER_AGENT, "infofetcher/0.1")
        .body(body)
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(AppError::Upstream(format!(
            "WebSub 订阅失败: {}",
            response.status()
        )));
    }
    Ok(true)
}

pub async fn verify_subscription(
    state: &AppState,
    feed_id: Uuid,
    mode: &str,
    topic: &str,
    lease_seconds: Option<i64>,
) -> AppResult<()> {
    if !matches!(mode, "subscribe" | "unsubscribe") {
        return Err(AppError::BadRequest("WebSub mode 无效".to_string()));
    }
    let row = sqlx::query("SELECT websub_topic_url FROM feeds WHERE id = $1")
        .bind(feed_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;
    if row.get::<Option<String>, _>("websub_topic_url").as_deref() != Some(topic) {
        return Err(AppError::BadRequest("WebSub topic 不匹配".to_string()));
    }
    if mode == "subscribe" {
        let lease = lease_seconds
            .unwrap_or(state.config.websub_lease_seconds)
            .clamp(300, 30 * 24 * 60 * 60);
        sqlx::query(
            "UPDATE feeds
             SET websub_lease_expires_at = now() + ($2 || ' seconds')::interval,
                 updated_at = now()
             WHERE id = $1",
        )
        .bind(feed_id)
        .bind(lease)
        .execute(&state.pool)
        .await?;
    } else {
        sqlx::query(
            "UPDATE feeds SET websub_lease_expires_at = NULL, updated_at = now() WHERE id = $1",
        )
        .bind(feed_id)
        .execute(&state.pool)
        .await?;
    }
    Ok(())
}

pub async fn receive_notification(
    state: &AppState,
    feed_id: Uuid,
    headers: &HeaderMap,
    body: &[u8],
) -> AppResult<()> {
    let row = sqlx::query("SELECT websub_secret FROM feeds WHERE id = $1")
        .bind(feed_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;
    let secret = row
        .get::<Option<String>, _>("websub_secret")
        .filter(|value| !value.is_empty())
        .ok_or(AppError::Unauthorized)?;
    let signature = headers
        .get("x-hub-signature-256")
        .or_else(|| headers.get("x-hub-signature"))
        .and_then(|value| value.to_str().ok())
        .ok_or(AppError::Unauthorized)?;
    if !verify_signature(&secret, signature, body) {
        return Err(AppError::Unauthorized);
    }
    worker::enqueue_worker_command(state, WorkerCommand::FetchFeed(feed_id)).await?;
    Ok(())
}

fn verify_signature(secret: &str, signature: &str, body: &[u8]) -> bool {
    let Some((algorithm, expected)) = signature.split_once('=') else {
        return false;
    };
    let Ok(expected) = hex::decode(expected) else {
        return false;
    };
    match algorithm.to_ascii_lowercase().as_str() {
        "sha256" => Hmac::<Sha256>::new_from_slice(secret.as_bytes()).is_ok_and(|mut mac| {
            mac.update(body);
            mac.verify_slice(&expected).is_ok()
        }),
        "sha1" => Hmac::<Sha1>::new_from_slice(secret.as_bytes()).is_ok_and(|mut mac| {
            mac.update(body);
            mac.verify_slice(&expected).is_ok()
        }),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_sha256_signature() {
        let mut mac = Hmac::<Sha256>::new_from_slice(b"secret").unwrap();
        mac.update(b"payload");
        let signature = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
        assert!(verify_signature("secret", &signature, b"payload"));
        assert!(!verify_signature("secret", &signature, b"changed"));
    }
}
