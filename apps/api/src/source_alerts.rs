//! 信息源健康告警：连续失败达到阈值、以及失败后恢复时各推送一次 Telegram。
//! 只在状态切换点触发，避免每次重试都刷屏；未配置 Telegram 时只记录日志。

use sqlx::Row;
use uuid::Uuid;

use crate::{push::send_telegram, state::AppState};

pub const FEED_FAILURE_ALERT_THRESHOLD: i32 = 3;
const ERROR_PREVIEW_CHARS: usize = 300;

pub fn should_alert_failure(consecutive_failures: i32) -> bool {
    consecutive_failures == FEED_FAILURE_ALERT_THRESHOLD
}

pub fn should_alert_recovery(previous_failures: i32) -> bool {
    previous_failures >= FEED_FAILURE_ALERT_THRESHOLD
}

pub fn failure_message(title: &str, failures: i32, error: &str) -> String {
    format!(
        "信息源连续失败：{title}\n已连续失败 {failures} 次，系统会按退避计划继续重试。\n最近错误：{}",
        clip(error, ERROR_PREVIEW_CHARS)
    )
}

pub fn recovery_message(title: &str, previous_failures: i32) -> String {
    format!("信息源已恢复：{title}\n此前连续失败 {previous_failures} 次。")
}

/// 在事务提交后调用；推送在后台执行，失败只记日志，不影响抓取结果。
pub fn spawn_feed_alert(state: &AppState, feed_id: Uuid, kind: FeedAlert) {
    let state = state.clone();
    tokio::spawn(async move {
        let title = sqlx::query("SELECT title FROM feeds WHERE id = $1")
            .bind(feed_id)
            .fetch_optional(&state.pool)
            .await
            .ok()
            .flatten()
            .map(|row| row.get::<String, _>("title"))
            .unwrap_or_else(|| feed_id.to_string());
        let message = match &kind {
            FeedAlert::Failing { failures, error } => failure_message(&title, *failures, error),
            FeedAlert::Recovered { previous_failures } => {
                recovery_message(&title, *previous_failures)
            }
        };
        if state.config.telegram_bot_token.trim().is_empty()
            || state.config.telegram_chat_id.trim().is_empty()
        {
            tracing::warn!(%feed_id, %message, "信息源告警未推送：Telegram 未配置");
            return;
        }
        if let Err(error) = send_telegram(&state.config, &state.http, &message).await {
            tracing::warn!(%feed_id, %error, "信息源告警推送失败");
        }
    });
}

#[derive(Debug, Clone)]
pub enum FeedAlert {
    Failing { failures: i32, error: String },
    Recovered { previous_failures: i32 },
}

fn clip(value: &str, limit: usize) -> String {
    let mut clipped = value.chars().take(limit).collect::<String>();
    if value.chars().count() > limit {
        clipped.push_str("...");
    }
    clipped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alerts_only_on_state_transitions() {
        assert!(!should_alert_failure(2));
        assert!(should_alert_failure(3));
        assert!(!should_alert_failure(4));
        assert!(!should_alert_failure(59));
        assert!(!should_alert_recovery(0));
        assert!(!should_alert_recovery(2));
        assert!(should_alert_recovery(3));
        assert!(should_alert_recovery(59));
    }

    #[test]
    fn formats_messages_and_clips_long_errors() {
        let message = failure_message("X · 我的关注", 3, &"e".repeat(400));
        assert!(message.starts_with("信息源连续失败：X · 我的关注\n已连续失败 3 次"));
        assert!(message.ends_with("..."));
        assert_eq!(
            recovery_message("BBC", 59),
            "信息源已恢复：BBC\n此前连续失败 59 次。"
        );
    }
}
