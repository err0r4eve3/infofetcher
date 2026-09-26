use std::collections::HashSet;

use chrono::{DateTime, Duration, Utc};
use sqlx::Row;
use tokio::time;
use uuid::Uuid;

use crate::{
    error::AppResult,
    intelligence::{
        lock_story_generation_for_metadata_write, refresh_story_clusters_after_source_change,
    },
    state::AppState,
};

const RETENTION_BATCH_SIZE: i64 = 5_000;
const RETENTION_INITIAL_DELAY: time::Duration = time::Duration::from_secs(60);
const RETENTION_INTERVAL: time::Duration = time::Duration::from_secs(6 * 60 * 60);

#[derive(Debug, Default)]
pub struct RetentionStats {
    pub articles: u64,
    pub stories: u64,
    pub worker_jobs: u64,
    pub event_agent_runs: u64,
}

#[derive(Debug, Default)]
struct RetentionBatch {
    article_ids: Vec<Uuid>,
    deleted_stories: u64,
}

pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        time::sleep(RETENTION_INITIAL_DELAY).await;
        loop {
            match run_once(&state).await {
                Ok(stats) => tracing::info!(
                    retention_days = state.config.content_retention_days,
                    articles = stats.articles,
                    stories = stats.stories,
                    worker_jobs = stats.worker_jobs,
                    event_agent_runs = stats.event_agent_runs,
                    "scheduled content retention completed"
                ),
                Err(error) => tracing::warn!(error = %error, "scheduled content retention failed"),
            }
            time::sleep(RETENTION_INTERVAL).await;
        }
    });
}

pub async fn run_once(state: &AppState) -> AppResult<RetentionStats> {
    let retention_days = normalize_retention_days(state.config.content_retention_days);
    let cutoff = retention_cutoff(Utc::now(), retention_days);
    let mut stats = RetentionStats::default();

    loop {
        let batch = prune_article_batch(state, cutoff).await?;
        let deleted = batch.article_ids.len();
        if deleted == 0 {
            break;
        }
        state.search.delete_articles(&batch.article_ids)?;
        stats.articles += deleted as u64;
        stats.stories += batch.deleted_stories;
        if deleted < RETENTION_BATCH_SIZE as usize {
            break;
        }
    }

    let maintenance = prune_operational_history(state, cutoff).await?;
    stats.stories += maintenance.stories;
    stats.worker_jobs = maintenance.worker_jobs;
    stats.event_agent_runs = maintenance.event_agent_runs;
    Ok(stats)
}

async fn prune_article_batch(state: &AppState, cutoff: DateTime<Utc>) -> AppResult<RetentionBatch> {
    let mut transaction = state.pool.begin().await?;
    lock_story_generation_for_metadata_write(&mut transaction).await?;
    let rows = sqlx::query(
        "SELECT a.id, a.story_id
         FROM articles a
         WHERE COALESCE(a.published_at, a.fetched_at) < $1
           AND a.is_favorite = false
           AND NOT EXISTS (
             SELECT 1 FROM worker_jobs w
             WHERE w.article_id = a.id AND w.status IN ('pending', 'running')
           )
           AND (
             a.story_id IS NULL OR NOT EXISTS (
               SELECT 1 FROM event_agent_runs ear
               WHERE ear.story_id = a.story_id AND ear.status IN ('pending', 'running')
             )
           )
         ORDER BY COALESCE(a.published_at, a.fetched_at), a.id
         FOR UPDATE OF a SKIP LOCKED
         LIMIT $2",
    )
    .bind(cutoff)
    .bind(RETENTION_BATCH_SIZE)
    .fetch_all(&mut *transaction)
    .await?;
    if rows.is_empty() {
        transaction.rollback().await?;
        return Ok(RetentionBatch::default());
    }
    let article_ids = rows
        .iter()
        .map(|row| row.get::<Uuid, _>("id"))
        .collect::<Vec<_>>();
    let story_ids = rows
        .iter()
        .filter_map(|row| row.get::<Option<Uuid>, _>("story_id"))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    sqlx::query("DELETE FROM articles WHERE id = ANY($1)")
        .bind(&article_ids)
        .execute(&mut *transaction)
        .await?;
    refresh_story_clusters_after_source_change(&mut transaction, &story_ids, true).await?;
    let remaining_stories =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM story_clusters WHERE id = ANY($1)")
            .bind(&story_ids)
            .fetch_one(&mut *transaction)
            .await?;
    let deleted_stories = story_ids.len() as u64 - remaining_stories.max(0) as u64;
    transaction.commit().await?;
    Ok(RetentionBatch {
        article_ids,
        deleted_stories,
    })
}

#[derive(Debug, Default)]
struct MaintenanceStats {
    stories: u64,
    worker_jobs: u64,
    event_agent_runs: u64,
}

async fn prune_operational_history(
    state: &AppState,
    cutoff: DateTime<Utc>,
) -> AppResult<MaintenanceStats> {
    let mut transaction = state.pool.begin().await?;
    lock_story_generation_for_metadata_write(&mut transaction).await?;
    sqlx::query("DELETE FROM event_revisions WHERE occurred_at < $1")
        .bind(cutoff)
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        "DELETE FROM event_digests ed
         WHERE COALESCE(ed.published_at, ed.updated_at) < $1
           AND NOT EXISTS (
             SELECT 1 FROM articles a
             WHERE a.story_id = ed.story_id
               AND COALESCE(a.published_at, a.fetched_at) >= $1
           )",
    )
    .bind(cutoff)
    .execute(&mut *transaction)
    .await?;
    let event_agent_runs = sqlx::query(
        "DELETE FROM event_agent_runs
         WHERE status IN ('done', 'failed') AND updated_at < $1",
    )
    .bind(cutoff)
    .execute(&mut *transaction)
    .await?
    .rows_affected();
    let worker_jobs = sqlx::query(
        "DELETE FROM worker_jobs
         WHERE status IN ('done', 'failed') AND updated_at < $1",
    )
    .bind(cutoff)
    .execute(&mut *transaction)
    .await?
    .rows_affected();
    sqlx::query(
        "DELETE FROM summary_jobs
         WHERE status IN ('done', 'failed') AND updated_at < $1",
    )
    .bind(cutoff)
    .execute(&mut *transaction)
    .await?;
    sqlx::query("DELETE FROM sessions WHERE expires_at < now()")
        .execute(&mut *transaction)
        .await?;
    sqlx::query("DELETE FROM app_events WHERE created_at < $1")
        .bind(cutoff)
        .execute(&mut *transaction)
        .await?;
    let stories = sqlx::query(
        "DELETE FROM story_clusters sc
         WHERE NOT EXISTS (SELECT 1 FROM articles a WHERE a.story_id = sc.id)
           AND NOT EXISTS (SELECT 1 FROM event_feedback ef WHERE ef.story_id = sc.id)",
    )
    .execute(&mut *transaction)
    .await?
    .rows_affected();
    transaction.commit().await?;
    Ok(MaintenanceStats {
        stories,
        worker_jobs,
        event_agent_runs,
    })
}

fn retention_cutoff(now: DateTime<Utc>, days: i64) -> DateTime<Utc> {
    now - Duration::days(normalize_retention_days(days))
}

fn normalize_retention_days(days: i64) -> i64 {
    days.clamp(1, 365)
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    #[test]
    fn retention_cutoff_is_exactly_configured_days_old() {
        let now = Utc.with_ymd_and_hms(2026, 8, 26, 12, 0, 0).unwrap();
        assert_eq!(
            retention_cutoff(now, 7),
            Utc.with_ymd_and_hms(2026, 8, 19, 12, 0, 0).unwrap()
        );
    }

    #[test]
    fn retention_days_are_bounded() {
        assert_eq!(normalize_retention_days(0), 1);
        assert_eq!(normalize_retention_days(7), 7);
        assert_eq!(normalize_retention_days(999), 365);
    }
}
