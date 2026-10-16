//! 信源画像：用评审结果计算每个信源近 30 天的信噪比（评审达标的事件数 / 文章数）和均分，
//! 据此微调排序用的来源质量（±8），并在信噪比腰斩时告警一次。手填的 `quality_score` 不变。

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use sqlx::Row;
use uuid::Uuid;

use crate::{error::AppResult, push::send_telegram, state::AppState};

const WINDOW_DAYS: i64 = 30;
/// 文章太少的信源不计算信噪比。
const MIN_ARTICLES: i64 = 20;
/// 评审覆盖不足 3 天时不微调，避免冷启动时用一天的数据下结论。
const MIN_COVERAGE_DAYS: i64 = 3;
const MAX_ADJUST: i32 = 8;
const DECLINE_RATIO: f32 = 0.5;
const MIN_PREVIOUS_PASSED: i64 = 5;
const ALERT_COOLDOWN_DAYS: i64 = 30;

#[derive(Debug, Clone, PartialEq)]
struct FeedWindow {
    feed_id: Uuid,
    title: String,
    articles: i64,
    previous_articles: i64,
    assessed: i64,
    passed: i64,
    previous_passed: i64,
    negative: i64,
    average_score: Option<f32>,
    alerted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq)]
struct FeedStats {
    snr: Option<f32>,
    previous_snr: Option<f32>,
    auto_adjust: i32,
    declining: bool,
}

pub async fn refresh(state: &AppState) -> AppResult<usize> {
    let now = Utc::now();
    let Some(coverage_start) = sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
        "SELECT MIN(assessed_at) FROM event_assessments",
    )
    .fetch_one(&state.pool)
    .await?
    else {
        return Ok(0);
    };
    let window_start = coverage_start.max(now - Duration::days(WINDOW_DAYS));
    let previous_start = now - Duration::days(WINDOW_DAYS * 2);
    // 上一窗口必须完整落在评审覆盖期内，否则不做环比。
    let has_previous = coverage_start <= previous_start;
    let (profiles, thresholds): (Vec<String>, Vec<f32>) = state
        .config
        .editorial
        .thresholds
        .iter()
        .map(|(profile, threshold)| (profile.clone(), *threshold))
        .unzip();
    let rows = sqlx::query(
        "WITH th AS (SELECT unnest($1::text[]) AS profile, unnest($2::real[]) AS threshold),
         fa AS (
           SELECT a.feed_id, a.story_id, COALESCE(a.published_at, a.fetched_at) >= $3 AS current
           FROM articles a
           WHERE COALESCE(a.published_at, a.fetched_at) >= CASE WHEN $5 THEN $4 ELSE $3 END
         ),
         article_counts AS (
           SELECT feed_id,
                  COUNT(*) FILTER (WHERE current) AS articles,
                  COUNT(*) FILTER (WHERE NOT current) AS previous_articles
           FROM fa GROUP BY feed_id
         ),
         per_story AS (
           SELECT DISTINCT feed_id, story_id, current FROM fa WHERE story_id IS NOT NULL
         ),
         story_counts AS (
           SELECT ps.feed_id,
                  COUNT(*) FILTER (WHERE ps.current AND ea.score IS NOT NULL) AS assessed,
                  COUNT(*) FILTER (WHERE ps.current AND ea.score >= COALESCE(th.threshold, 5)) AS passed,
                  COUNT(*) FILTER (WHERE NOT ps.current AND ea.score >= COALESCE(th.threshold, 5)) AS previous_passed,
                  COUNT(*) FILTER (WHERE ps.current AND ef.kind IN ('irrelevant', 'less')) AS negative,
                  AVG(ea.score) FILTER (WHERE ps.current)::real AS average_score
           FROM per_story ps
           LEFT JOIN event_assessments ea
             ON ea.story_id = ps.story_id AND ea.status = 'done' AND ea.score IS NOT NULL
           LEFT JOIN th ON th.profile = ea.profile
           LEFT JOIN event_feedback ef ON ef.story_id = ps.story_id
           GROUP BY ps.feed_id
         )
         SELECT f.id AS feed_id, f.title,
                COALESCE(ac.articles, 0) AS articles,
                COALESCE(ac.previous_articles, 0) AS previous_articles,
                COALESCE(sc.assessed, 0) AS assessed, COALESCE(sc.passed, 0) AS passed,
                COALESCE(sc.previous_passed, 0) AS previous_passed,
                COALESCE(sc.negative, 0) AS negative, sc.average_score,
                fqs.alerted_at
         FROM feeds f
         LEFT JOIN article_counts ac ON ac.feed_id = f.id
         LEFT JOIN story_counts sc ON sc.feed_id = f.id
         LEFT JOIN feed_quality_stats fqs ON fqs.feed_id = f.id",
    )
    .bind(&profiles)
    .bind(&thresholds)
    .bind(window_start)
    .bind(previous_start)
    .bind(has_previous)
    .fetch_all(&state.pool)
    .await?;
    let windows = rows
        .into_iter()
        .map(|row| FeedWindow {
            feed_id: row.get("feed_id"),
            title: row.get("title"),
            articles: row.get("articles"),
            previous_articles: row.get("previous_articles"),
            assessed: row.get("assessed"),
            passed: row.get("passed"),
            previous_passed: row.get("previous_passed"),
            negative: row.get("negative"),
            average_score: row.get("average_score"),
            alerted_at: row.get("alerted_at"),
        })
        .collect::<Vec<_>>();
    let coverage_days = (now - window_start).num_days();
    let stats = compute(&windows, coverage_days >= MIN_COVERAGE_DAYS, has_previous);
    let window_days = coverage_days.clamp(0, WINDOW_DAYS) as i32;
    for window in &windows {
        let feed_stats = &stats[&window.feed_id];
        let alert = feed_stats.declining
            && window
                .alerted_at
                .is_none_or(|at| at < now - Duration::days(ALERT_COOLDOWN_DAYS));
        sqlx::query(
            "INSERT INTO feed_quality_stats (
               feed_id, window_days, article_count, assessed_events, passed_events,
               irrelevant_events, average_score, snr, previous_snr, auto_adjust, declining,
               alerted_at, computed_at
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11,
                       CASE WHEN $12 THEN now() ELSE NULL END, now())
             ON CONFLICT (feed_id) DO UPDATE SET
               window_days = EXCLUDED.window_days, article_count = EXCLUDED.article_count,
               assessed_events = EXCLUDED.assessed_events, passed_events = EXCLUDED.passed_events,
               irrelevant_events = EXCLUDED.irrelevant_events,
               average_score = EXCLUDED.average_score, snr = EXCLUDED.snr,
               previous_snr = EXCLUDED.previous_snr, auto_adjust = EXCLUDED.auto_adjust,
               declining = EXCLUDED.declining,
               alerted_at = CASE WHEN $12 THEN now() ELSE feed_quality_stats.alerted_at END,
               computed_at = now()",
        )
        .bind(window.feed_id)
        .bind(window_days)
        .bind(window.articles as i32)
        .bind(window.assessed as i32)
        .bind(window.passed as i32)
        .bind(window.negative as i32)
        .bind(window.average_score)
        .bind(feed_stats.snr)
        .bind(feed_stats.previous_snr)
        .bind(feed_stats.auto_adjust)
        .bind(feed_stats.declining)
        .bind(alert)
        .execute(&state.pool)
        .await?;
        if alert {
            send_decline_alert(state, window, feed_stats).await;
        }
    }
    Ok(windows.len())
}

fn snr(passed: i64, articles: i64) -> Option<f32> {
    (articles >= MIN_ARTICLES).then(|| passed as f32 / articles as f32)
}

/// 微调量相对全体信源的中位信噪比计算：高一倍 +4，低一半 -4，封顶 ±8。
fn compute(windows: &[FeedWindow], adjust: bool, has_previous: bool) -> HashMap<Uuid, FeedStats> {
    let mut ratios = windows
        .iter()
        .filter_map(|window| snr(window.passed, window.articles))
        .collect::<Vec<_>>();
    ratios.sort_by(f32::total_cmp);
    let median = ratios
        .get(ratios.len() / 2)
        .copied()
        .filter(|median| *median > 0.0);
    windows
        .iter()
        .map(|window| {
            let current = snr(window.passed, window.articles);
            let previous = has_previous
                .then(|| snr(window.previous_passed, window.previous_articles))
                .flatten();
            let auto_adjust = match (adjust, current, median) {
                (true, Some(ratio), Some(_)) if ratio <= 0.0 => -MAX_ADJUST,
                (true, Some(ratio), Some(median)) => {
                    ((ratio / median).log2() * 4.0).round().clamp(-8.0, 8.0) as i32
                }
                _ => 0,
            };
            let declining = matches!((current, previous), (Some(now), Some(before))
                if window.previous_passed >= MIN_PREVIOUS_PASSED && now < before * DECLINE_RATIO);
            (
                window.feed_id,
                FeedStats {
                    snr: current,
                    previous_snr: previous,
                    auto_adjust,
                    declining,
                },
            )
        })
        .collect()
}

pub fn decline_message(title: &str, previous: f32, current: f32) -> String {
    format!(
        "信息源质量下降：{title}\n近 30 天信噪比从 {:.0}% 降到 {:.0}%。可在信源管理中检查或静音。",
        previous * 100.0,
        current * 100.0
    )
}

async fn send_decline_alert(state: &AppState, window: &FeedWindow, stats: &FeedStats) {
    let (Some(previous), Some(current)) = (stats.previous_snr, stats.snr) else {
        return;
    };
    let message = decline_message(&window.title, previous, current);
    if state.config.telegram_bot_token.trim().is_empty()
        || state.config.telegram_chat_id.trim().is_empty()
    {
        tracing::warn!(feed_id = %window.feed_id, %message, "信源质量告警未推送：Telegram 未配置");
        return;
    }
    if let Err(error) = send_telegram(&state.config, &state.http, &message).await {
        tracing::warn!(feed_id = %window.feed_id, %error, "信源质量告警推送失败");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(
        passed: i64,
        articles: i64,
        previous_passed: i64,
        previous_articles: i64,
    ) -> FeedWindow {
        FeedWindow {
            feed_id: Uuid::new_v4(),
            title: "Feed".to_string(),
            articles,
            previous_articles,
            assessed: passed,
            passed,
            previous_passed,
            negative: 0,
            average_score: None,
            alerted_at: None,
        }
    }

    #[test]
    fn adjusts_relative_to_the_median_feed() {
        let windows = vec![
            window(10, 100, 0, 0),
            window(20, 100, 0, 0),
            window(40, 100, 0, 0),
            window(0, 100, 0, 0),
            window(5, 10, 0, 0),
        ];
        let stats = compute(&windows, true, false);
        assert_eq!(stats[&windows[0].feed_id].auto_adjust, -4);
        assert_eq!(stats[&windows[1].feed_id].auto_adjust, 0);
        assert_eq!(stats[&windows[2].feed_id].auto_adjust, 4);
        assert_eq!(stats[&windows[3].feed_id].auto_adjust, -8);
        assert_eq!(stats[&windows[4].feed_id].snr, None, "too few articles");
        assert_eq!(stats[&windows[4].feed_id].auto_adjust, 0);
    }

    #[test]
    fn cold_start_does_not_adjust() {
        let windows = vec![window(1, 100, 0, 0), window(50, 100, 0, 0)];
        assert!(
            compute(&windows, false, false)
                .values()
                .all(|stats| stats.auto_adjust == 0)
        );
    }

    #[test]
    fn flags_a_halved_signal_only_with_enough_history() {
        let windows = vec![
            window(10, 100, 30, 100),
            window(10, 100, 3, 10),
            window(10, 100, 30, 100),
        ];
        let stats = compute(&windows, true, true);
        assert!(stats[&windows[0].feed_id].declining);
        assert!(
            !stats[&windows[1].feed_id].declining,
            "previous window too small"
        );
        assert!(!compute(&windows, true, false)[&windows[2].feed_id].declining);
    }

    #[test]
    fn decline_message_reports_percentages() {
        assert_eq!(
            decline_message("某源", 0.3, 0.1),
            "信息源质量下降：某源\n近 30 天信噪比从 30% 降到 10%。可在信源管理中检查或静音。"
        );
    }
}
