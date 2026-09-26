use std::collections::HashSet;

use chrono::{DateTime, Utc};
use futures::{StreamExt, stream};
use reqwest::header::{
    ETAG, HeaderMap, HeaderValue, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED, USER_AGENT,
};
use sqlx::{Postgres, Row, Transaction};
use tokio::{sync::mpsc, task::JoinSet, time};
use uuid::Uuid;

use crate::{
    config::Config,
    error::{AppError, AppResult},
    extractor::{best_article_text, clean_extracted_text, extract_article_content},
    feeds::{canonicalize_url, hash_url, parse_feed_bytes},
    hub,
    intelligence::{
        article_quality_score, assign_story_cluster, detect_language, hash_content,
        lock_story_generation_for_article_write, lock_story_generation_for_metadata_write,
        publisher_family, refresh_stories_for_feed_change,
    },
    keywords,
    llm::{SummarySource, summarize_sources},
    models::SearchDocument,
    net::{get_with_safe_redirects, read_response_limited},
    state::AppState,
    websub,
};

#[derive(Debug, Clone)]
pub enum WorkerCommand {
    FetchFeed(Uuid),
    SummarizeArticle(Uuid),
}

struct UpsertedArticle {
    id: Uuid,
    should_index: bool,
}

struct WorkerJob {
    id: Uuid,
    run_token: Uuid,
    command: WorkerCommand,
    attempts: i32,
    max_attempts: i32,
}

#[derive(Clone)]
struct SummaryMaterial {
    source: SummarySource,
    feed_id: Uuid,
    content_hash: String,
    source_family: String,
}

const WORKER_ADVISORY_LOCK_KEY: i64 = 0x5253_5348_5542_0001;

pub fn spawn_worker(state: AppState, mut rx: mpsc::Receiver<WorkerCommand>) {
    tokio::spawn(async move {
        loop {
            let mut lock_connection = match state.pool.acquire().await {
                Ok(connection) => connection,
                Err(err) => {
                    tracing::warn!(error = %err, "worker leader connection failed");
                    time::sleep(time::Duration::from_secs(5)).await;
                    continue;
                }
            };
            if let Err(err) = sqlx::query("SELECT pg_advisory_lock($1)")
                .bind(WORKER_ADVISORY_LOCK_KEY)
                .fetch_one(&mut *lock_connection)
                .await
            {
                tracing::warn!(error = %err, "worker leader lock failed");
                time::sleep(time::Duration::from_secs(5)).await;
                continue;
            }
            tracing::info!("worker leader lock acquired");
            if let Err(err) = recover_stale_jobs(&state).await {
                tracing::warn!(error = %err, "stale worker job recovery failed");
            }
            if let Err(err) = repair_unclustered_articles(&state).await {
                tracing::warn!(error = %err, "unclustered article recovery failed");
            }
            let mut tasks = JoinSet::new();
            let mut heartbeat_interval = time::interval(time::Duration::from_secs(1));
            let mut work_interval = time::interval(time::Duration::from_secs(5));
            loop {
                tokio::select! {
                    command = rx.recv() => {
                        let Some(_command) = command else {
                            tasks.abort_all();
                            return;
                        };
                        if let Err(err) = process_due_jobs(&state, &mut tasks).await {
                            tracing::warn!(error = %err, "worker job drain failed");
                        }
                    }
                    result = tasks.join_next(), if !tasks.is_empty() => {
                        if let Some(Err(err)) = result {
                            tracing::warn!(error = %err, "worker task terminated unexpectedly");
                        }
                    }
                    _ = heartbeat_interval.tick() => {
                        if let Err(err) = sqlx::query("SELECT 1")
                            .fetch_one(&mut *lock_connection)
                            .await
                        {
                            tracing::warn!(error = %err, "worker leader connection lost");
                            tasks.abort_all();
                            while tasks.join_next().await.is_some() {}
                            break;
                        }
                    }
                    _ = work_interval.tick() => {
                        if let Err(err) = enqueue_due_feeds(&state).await {
                            tracing::warn!(error = %err, "periodic feed enqueue failed");
                        }
                        if let Err(err) = process_due_jobs(&state, &mut tasks).await {
                            tracing::warn!(error = %err, "periodic worker drain failed");
                        }
                    }
                }
            }
        }
    });
}

pub async fn enqueue_worker_command(state: &AppState, command: WorkerCommand) -> AppResult<bool> {
    let result = match command {
        WorkerCommand::FetchFeed(feed_id) => {
            sqlx::query(
                "INSERT INTO worker_jobs (kind, feed_id)
                 VALUES ('fetch_feed', $1)
                 ON CONFLICT DO NOTHING",
            )
            .bind(feed_id)
            .execute(&state.pool)
            .await?
        }
        WorkerCommand::SummarizeArticle(article_id) => {
            sqlx::query(
                "INSERT INTO worker_jobs (kind, article_id)
                 VALUES ('summarize_article', $1)
                 ON CONFLICT DO NOTHING",
            )
            .bind(article_id)
            .execute(&state.pool)
            .await?
        }
    };
    let inserted = result.rows_affected() > 0;
    let _ = state.worker_tx.try_send(command);
    Ok(inserted)
}

async fn handle_command(state: &AppState, job: &WorkerJob) -> AppResult<()> {
    match &job.command {
        WorkerCommand::FetchFeed(feed_id) => fetch_feed_by_id(state, *feed_id).await,
        WorkerCommand::SummarizeArticle(article_id) => {
            summarize_article_by_id(state, *article_id, job.run_token).await
        }
    }
}

async fn recover_stale_jobs(state: &AppState) -> AppResult<()> {
    sqlx::query(
        "UPDATE worker_jobs
         SET status = 'pending', available_at = now(), locked_at = NULL,
             run_token = NULL,
             last_error = COALESCE(last_error, '进程重启后恢复'), updated_at = now()
         WHERE status = 'running'",
    )
    .execute(&state.pool)
    .await?;
    sqlx::query(
        "UPDATE summary_jobs
         SET status = 'pending', available_at = now(),
             error = COALESCE(error, '进程重启后恢复'), updated_at = now()
         WHERE status = 'running'",
    )
    .execute(&state.pool)
    .await?;
    sqlx::query(
        "UPDATE summaries SET status = 'pending', run_token = NULL, updated_at = now()
         WHERE status = 'running'",
    )
    .execute(&state.pool)
    .await?;
    Ok(())
}

async fn repair_unclustered_articles(state: &AppState) -> AppResult<()> {
    let rows = sqlx::query(
        "SELECT id
         FROM articles
         WHERE story_id IS NULL OR needs_recluster
         ORDER BY COALESCE(published_at, fetched_at), fetched_at
         LIMIT 500",
    )
    .fetch_all(&state.pool)
    .await?;
    for row in rows {
        let article_id = row.get::<Uuid, _>("id");
        if let Err(err) = assign_story_cluster(state, article_id).await {
            tracing::warn!(article_id = %article_id, error = %err, "unclustered article repair failed");
        }
    }
    Ok(())
}

async fn process_due_jobs(state: &AppState, tasks: &mut JoinSet<()>) -> AppResult<()> {
    let fetch_concurrency = state.config.fetch_concurrency.min(32);
    let running_fetches = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM worker_jobs
         WHERE kind = 'fetch_feed' AND status = 'running'",
    )
    .fetch_one(&state.pool)
    .await?;
    let available_fetch_slots = fetch_concurrency.saturating_sub(running_fetches.max(0) as usize);
    for _ in 0..available_fetch_slots {
        let Some(job) = claim_next_job(state, Some("fetch_feed")).await? else {
            break;
        };
        let task_state = state.clone();
        tasks.spawn(async move {
            if let Err(err) = run_claimed_job(&task_state, job).await {
                tracing::warn!(error = %err, "claimed feed job failed");
            }
        });
    }

    let running_summaries = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM worker_jobs
         WHERE kind = 'summarize_article' AND status = 'running'",
    )
    .fetch_one(&state.pool)
    .await?;
    let available_summary_slots = state
        .config
        .llm_max_concurrency
        .saturating_sub(running_summaries.max(0) as usize);
    for _ in 0..available_summary_slots {
        let Some(job) = claim_next_job(state, Some("summarize_article")).await? else {
            break;
        };
        let task_state = state.clone();
        tasks.spawn(async move {
            if let Err(err) = run_claimed_job(&task_state, job).await {
                tracing::warn!(error = %err, "claimed summary job failed");
            }
        });
    }
    Ok(())
}

async fn claim_next_job(state: &AppState, kind: Option<&str>) -> AppResult<Option<WorkerJob>> {
    let row = sqlx::query(
        "WITH candidate AS (
           SELECT id
           FROM worker_jobs
           WHERE status = 'pending' AND available_at <= now()
             AND ($1::text IS NULL OR kind = $1)
           ORDER BY available_at, created_at
           FOR UPDATE SKIP LOCKED
           LIMIT 1
         )
         UPDATE worker_jobs j
         SET status = 'running', locked_at = now(), attempts = attempts + 1,
             run_token = gen_random_uuid(), updated_at = now()
         FROM candidate
         WHERE j.id = candidate.id
         RETURNING j.id, j.run_token, j.kind, j.feed_id, j.article_id,
                   j.attempts, j.max_attempts",
    )
    .bind(kind)
    .fetch_optional(&state.pool)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let kind = row.get::<String, _>("kind");
    let command = match kind.as_str() {
        "fetch_feed" => WorkerCommand::FetchFeed(row.get("feed_id")),
        "summarize_article" => WorkerCommand::SummarizeArticle(row.get("article_id")),
        _ => return Err(AppError::BadRequest(format!("未知 worker job: {kind}"))),
    };
    Ok(Some(WorkerJob {
        id: row.get("id"),
        run_token: row.get("run_token"),
        command,
        attempts: row.get("attempts"),
        max_attempts: row.get("max_attempts"),
    }))
}

async fn run_claimed_job(state: &AppState, job: WorkerJob) -> AppResult<()> {
    match handle_command(state, &job).await {
        Ok(()) => {
            let mut transaction = state.pool.begin().await?;
            let lease_update = sqlx::query(
                "UPDATE worker_jobs
                 SET status = 'done', completed_at = now(), locked_at = NULL,
                     run_token = NULL, last_error = NULL, updated_at = now()
                 WHERE id = $1 AND run_token = $2",
            )
            .bind(job.id)
            .bind(job.run_token)
            .execute(&mut *transaction)
            .await?;
            if lease_update.rows_affected() != 1 {
                transaction.rollback().await?;
                return Err(AppError::Upstream(
                    "worker 任务租约已失效，拒绝提交旧结果".to_string(),
                ));
            }
            let mut requeued_article = None;
            if let WorkerCommand::SummarizeArticle(article_id) = &job.command {
                let article_id = *article_id;
                sqlx::query(
                    "UPDATE summary_jobs
                     SET status = 'done', completed_at = now(), error = NULL, updated_at = now()
                     WHERE article_id = $1 AND status = 'running'",
                )
                .bind(article_id)
                .execute(&mut *transaction)
                .await?;
                let status =
                    sqlx::query("SELECT status FROM summaries WHERE article_id = $1 FOR UPDATE")
                        .bind(article_id)
                        .fetch_optional(&mut *transaction)
                        .await?
                        .map(|row| row.get::<String, _>("status"));
                if status.as_deref() == Some("pending") {
                    sqlx::query(
                        "INSERT INTO summary_jobs (article_id, scope, status)
                         VALUES ($1, 'article', 'pending')
                         ON CONFLICT DO NOTHING",
                    )
                    .bind(article_id)
                    .execute(&mut *transaction)
                    .await?;
                    let inserted = sqlx::query(
                        "INSERT INTO worker_jobs (kind, article_id)
                         VALUES ('summarize_article', $1)
                         ON CONFLICT DO NOTHING",
                    )
                    .bind(article_id)
                    .execute(&mut *transaction)
                    .await?
                    .rows_affected()
                        > 0;
                    if inserted {
                        requeued_article = Some(article_id);
                    }
                } else {
                    sqlx::query(
                        "UPDATE summaries
                         SET status = 'done', run_token = NULL, error = NULL, updated_at = now()
                         WHERE article_id = $1 AND status = 'running' AND run_token = $2",
                    )
                    .bind(article_id)
                    .bind(job.run_token)
                    .execute(&mut *transaction)
                    .await?;
                }
            }
            transaction.commit().await?;
            if let Some(article_id) = requeued_article {
                let _ = state
                    .worker_tx
                    .try_send(WorkerCommand::SummarizeArticle(article_id));
            }
            Ok(())
        }
        Err(err) => {
            let defer_to_feed_schedule = matches!(&job.command, WorkerCommand::FetchFeed(_))
                && feed_failure_uses_schedule(&err);
            let terminal =
                job_failure_is_terminal(&job.command, job.attempts, job.max_attempts, &err);
            let delay_seconds = retry_delay_seconds(job.id, job.attempts);
            let mut transaction = state.pool.begin().await?;
            let lease_update = sqlx::query(
                "UPDATE worker_jobs
                 SET status = $2,
                     available_at = CASE WHEN $2 = 'pending' THEN now() + ($3 || ' seconds')::interval ELSE available_at END,
                     locked_at = NULL, run_token = NULL, last_error = $4, updated_at = now(),
                     completed_at = CASE WHEN $2 = 'failed' THEN now() ELSE NULL END
                 WHERE id = $1 AND run_token = $5",
            )
            .bind(job.id)
            .bind(if terminal { "failed" } else { "pending" })
            .bind(delay_seconds)
            .bind(err.to_string())
            .bind(job.run_token)
            .execute(&mut *transaction)
            .await?;
            if lease_update.rows_affected() != 1 {
                transaction.rollback().await?;
                return Err(AppError::Upstream(
                    "worker 任务租约已失效，拒绝记录旧失败".to_string(),
                ));
            }
            record_command_failure(
                &mut transaction,
                &job.command,
                &err,
                terminal,
                delay_seconds,
                job.run_token,
                defer_to_feed_schedule,
            )
            .await?;
            transaction.commit().await?;
            Err(err)
        }
    }
}

fn job_failure_is_terminal(
    command: &WorkerCommand,
    attempts: i32,
    max_attempts: i32,
    error: &AppError,
) -> bool {
    (matches!(command, WorkerCommand::FetchFeed(_)) && feed_failure_uses_schedule(error))
        || attempts >= max_attempts
        || is_permanent_job_error(error)
}

fn feed_failure_uses_schedule(error: &AppError) -> bool {
    matches!(
        error,
        AppError::Unauthorized
            | AppError::NotFound
            | AppError::BadRequest(_)
            | AppError::RateLimited(_)
            | AppError::Upstream(_)
            | AppError::PermanentUpstream(_)
            | AppError::Reqwest(_)
    )
}

fn is_permanent_job_error(error: &AppError) -> bool {
    matches!(
        error,
        AppError::BadRequest(_)
            | AppError::PermanentUpstream(_)
            | AppError::NotFound
            | AppError::Unauthorized
    )
}

fn retry_delay_seconds(job_id: Uuid, attempts: i32) -> i64 {
    let exponent = attempts.saturating_sub(1).clamp(0, 7) as u32;
    let base = (30_i64.saturating_mul(2_i64.pow(exponent))).min(3_600);
    let jitter_per_mille = 800 + (job_id.as_u128() % 201) as i64;
    base.saturating_mul(jitter_per_mille) / 1_000
}

async fn record_command_failure(
    transaction: &mut Transaction<'_, Postgres>,
    command: &WorkerCommand,
    err: &AppError,
    terminal: bool,
    delay_seconds: i64,
    run_token: Uuid,
    defer_to_feed_schedule: bool,
) -> AppResult<()> {
    match command {
        WorkerCommand::FetchFeed(feed_id) => {
            if defer_to_feed_schedule {
                let failures = sqlx::query(
                    "UPDATE feeds
                     SET last_fetched_at = now(), last_error = $2,
                         consecutive_failures = consecutive_failures + 1,
                         next_fetch_at = now() + (LEAST(86400, 60 * power(2, LEAST(consecutive_failures, 11))) || ' seconds')::interval,
                         updated_at = now()
                     WHERE id = $1
                     RETURNING consecutive_failures",
                )
                .bind(feed_id)
                .bind(err.to_string())
                .fetch_optional(&mut **transaction)
                .await?;
                if let Some(row) = failures {
                    tracing::warn!(feed_id = %feed_id, failures = row.get::<i32, _>("consecutive_failures"), error = %err, "feed failure recorded");
                }
            } else if terminal {
                sqlx::query(
                    "UPDATE feeds
                     SET last_fetched_at = now(), last_error = $2,
                         next_fetch_at = now() + interval '5 minutes', updated_at = now()
                     WHERE id = $1",
                )
                .bind(feed_id)
                .bind(err.to_string())
                .execute(&mut **transaction)
                .await?;
            }
        }
        WorkerCommand::SummarizeArticle(article_id) => {
            let next_status = if terminal { "failed" } else { "pending" };
            sqlx::query(
                "UPDATE summary_jobs
                 SET status = $3, error = $2, attempts = attempts + 1,
                     available_at = CASE WHEN $3 = 'pending' THEN now() + ($4 || ' seconds')::interval ELSE available_at END,
                     completed_at = CASE WHEN $3 = 'failed' THEN now() ELSE NULL END,
                     updated_at = now()
                 WHERE article_id = $1 AND status IN ('pending', 'running')",
            )
            .bind(article_id)
            .bind(err.to_string())
            .bind(next_status)
            .bind(delay_seconds)
            .execute(&mut **transaction)
            .await?;
            sqlx::query(
                "UPDATE summaries
                 SET status = $3, run_token = NULL, error = $2, updated_at = now()
                 WHERE article_id = $1 AND run_token = $4",
            )
            .bind(article_id)
            .bind(err.to_string())
            .bind(next_status)
            .bind(run_token)
            .execute(&mut **transaction)
            .await?;
        }
    }
    Ok(())
}

pub async fn fetch_all_once(state: &AppState) -> AppResult<()> {
    let rows = sqlx::query("SELECT id FROM feeds ORDER BY title")
        .fetch_all(&state.pool)
        .await?;
    let ids = rows
        .into_iter()
        .map(|row| row.get::<Uuid, _>("id"))
        .collect::<Vec<_>>();
    fetch_many(state, ids).await;
    Ok(())
}

async fn enqueue_due_feeds(state: &AppState) -> AppResult<()> {
    let rows = sqlx::query(
        "SELECT id FROM feeds
         WHERE COALESCE(next_fetch_at, last_fetched_at, '-infinity'::timestamptz) <= now()
         ORDER BY next_fetch_at ASC NULLS FIRST, last_fetched_at ASC NULLS FIRST
         LIMIT 200",
    )
    .fetch_all(&state.pool)
    .await?;
    for row in rows {
        enqueue_worker_command(state, WorkerCommand::FetchFeed(row.get("id"))).await?;
    }
    Ok(())
}

async fn fetch_many(state: &AppState, ids: Vec<Uuid>) {
    stream::iter(ids)
        .for_each_concurrent(state.config.fetch_concurrency, |feed_id| async move {
            if let Err(err) = fetch_feed_by_id(state, feed_id).await {
                tracing::warn!(feed_id = %feed_id, error = %err, "feed fetch failed");
            }
        })
        .await;
}

fn internal_hub_source(
    adapter: Option<String>,
    path: Option<String>,
) -> AppResult<Option<(String, String)>> {
    match (adapter, path) {
        (None, None) => Ok(None),
        (Some(adapter), Some(path)) if !adapter.trim().is_empty() && !path.trim().is_empty() => {
            Ok(Some((adapter, path)))
        }
        _ => Err(AppError::BadRequest(
            "Hub 订阅缺少 adapter 或 path".to_string(),
        )),
    }
}

pub async fn fetch_feed_by_id(state: &AppState, feed_id: Uuid) -> AppResult<()> {
    let row = sqlx::query(
        "SELECT id, title, site_url, feed_url, hub_adapter, hub_path, etag, last_modified
         FROM feeds WHERE id = $1",
    )
    .bind(feed_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    let previous_title = row.get::<String, _>("title");
    let previous_site_url = row.get::<Option<String>, _>("site_url");
    let feed_url = row.get::<String, _>("feed_url");
    let hub_source = internal_hub_source(
        row.get::<Option<String>, _>("hub_adapter"),
        row.get::<Option<String>, _>("hub_path"),
    )?;
    let (body, etag, last_modified) = if let Some((adapter, path)) = hub_source {
        let generated = hub::generate_feed_bounded(
            &state.config,
            &state.http,
            &state.opencli_semaphore,
            &adapter,
            &path,
        )
        .await?;
        (hub::render_rss(generated).into_bytes(), None, None)
    } else {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("infofetcher/0.1"));
        if let Some(etag) = row.get::<Option<String>, _>("etag")
            && let Ok(value) = HeaderValue::from_str(&etag)
        {
            headers.insert(IF_NONE_MATCH, value);
        }
        if let Some(last_modified) = row.get::<Option<String>, _>("last_modified")
            && let Ok(value) = HeaderValue::from_str(&last_modified)
        {
            headers.insert(IF_MODIFIED_SINCE, value);
        }

        let response = get_with_safe_redirects(
            &state.http,
            &feed_url,
            &headers,
            state.config.allow_private_fetch,
        )
        .await?;
        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            sqlx::query(
                "UPDATE feeds
                 SET last_fetched_at = now(), last_success_at = now(), last_error = NULL,
                     consecutive_failures = 0,
                     next_fetch_at = now() + (fetch_interval_minutes || ' minutes')::interval,
                     updated_at = now()
                 WHERE id = $1",
            )
            .bind(feed_id)
            .execute(&state.pool)
            .await?;
            return Ok(());
        }
        if !response.status().is_success() {
            return Err(AppError::Upstream(format!(
                "Feed 请求失败: {}",
                response.status()
            )));
        }

        let etag = response
            .headers()
            .get(ETAG)
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);
        let last_modified = response
            .headers()
            .get(LAST_MODIFIED)
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);
        let body = read_response_limited(response, state.config.feed_max_bytes).await?;
        (body, etag, last_modified)
    };
    let parsed = parse_feed_bytes(&body, &feed_url)?;

    let next_site_url = parsed.site_url.as_ref().or(previous_site_url.as_ref());
    let publisher_metadata_changed =
        previous_title != parsed.title || previous_site_url.as_ref() != next_site_url;
    let mut transaction = state.pool.begin().await?;
    if publisher_metadata_changed {
        lock_story_generation_for_metadata_write(&mut transaction).await?;
    }
    sqlx::query(
        "UPDATE feeds
         SET title = $2, site_url = COALESCE($3, site_url),
             language = COALESCE($4, language),
             websub_hub_url = COALESCE($5, websub_hub_url),
             websub_topic_url = COALESCE($6, websub_topic_url),
             fetch_interval_minutes = COALESCE($7, fetch_interval_minutes),
             updated_at = now()
         WHERE id = $1",
    )
    .bind(feed_id)
    .bind(&parsed.title)
    .bind(&parsed.site_url)
    .bind(&parsed.language)
    .bind(&parsed.websub_hub_url)
    .bind(&parsed.websub_topic_url)
    .bind(parsed.ttl_minutes.map(|ttl| ttl.clamp(5, 1_440)))
    .execute(&mut *transaction)
    .await?;
    if publisher_metadata_changed {
        refresh_stories_for_feed_change(&mut transaction, feed_id, true).await?;
    }
    transaction.commit().await?;

    let mut search_documents = Vec::new();
    let mut changed_article_ids = Vec::new();
    for item in parsed.items {
        if let Some(existing_id) = existing_article_matches_feed_item(
            state,
            feed_id,
            &item.guid,
            &item.url,
            &item.title,
            item.author.as_deref(),
            &item.content,
            item.published_at,
        )
        .await?
        {
            ensure_story_cluster(state, existing_id).await?;
            changed_article_ids.push(existing_id);
            search_documents.push(load_search_document(state, existing_id, &parsed.title).await?);
            continue;
        }

        let cookie_header = state.config.article_fetch_cookie_for(&item.url);
        let extracted = extract_article_content(
            &state.config,
            &state.http,
            &item.url,
            &item.content,
            cookie_header,
        )
        .await;
        let upserted = upsert_article(
            state,
            feed_id,
            &parsed.title,
            &item.guid,
            &item.url,
            &item.title,
            item.author.as_deref(),
            &item.content,
            &extracted.text,
            extracted.error.as_deref(),
            item.published_at,
        )
        .await?;
        if upserted.should_index {
            assign_story_cluster(state, upserted.id).await?;
            changed_article_ids.push(upserted.id);
            search_documents.push(SearchDocument {
                id: upserted.id,
                feed_title: parsed.title.clone(),
                title: item.title,
                content: extracted.text,
                url: item.url,
                published_at: item.published_at,
            });
        }
    }

    state.search.index_articles(&search_documents)?;
    for article_id in changed_article_ids {
        keywords::process_article_keyword_rules(state, article_id).await?;
    }

    sqlx::query(
        "UPDATE feeds
         SET etag = $2, last_modified = $3,
             last_fetched_at = now(), last_success_at = now(), last_error = NULL,
             consecutive_failures = 0,
             next_fetch_at = now() + (fetch_interval_minutes || ' minutes')::interval,
             updated_at = now()
         WHERE id = $1",
    )
    .bind(feed_id)
    .bind(&etag)
    .bind(&last_modified)
    .execute(&state.pool)
    .await?;

    if let Err(err) = websub::maybe_subscribe(state, feed_id).await {
        tracing::warn!(feed_id = %feed_id, error = %err, "WebSub subscription failed");
    }

    Ok(())
}

async fn load_search_document(
    state: &AppState,
    article_id: Uuid,
    feed_title: &str,
) -> AppResult<SearchDocument> {
    let row = sqlx::query(
        "SELECT id, title, extracted_content, content, url, published_at
         FROM articles WHERE id = $1",
    )
    .bind(article_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    let extracted = row.get::<String, _>("extracted_content");
    Ok(SearchDocument {
        id: row.get("id"),
        feed_title: feed_title.to_string(),
        title: row.get("title"),
        content: if extracted.trim().is_empty() {
            row.get("content")
        } else {
            extracted
        },
        url: row.get("url"),
        published_at: row.get("published_at"),
    })
}

async fn ensure_story_cluster(state: &AppState, article_id: Uuid) -> AppResult<()> {
    let row = sqlx::query("SELECT story_id, needs_recluster FROM articles WHERE id = $1")
        .bind(article_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;
    if row.get::<Option<Uuid>, _>("story_id").is_none() || row.get::<bool, _>("needs_recluster") {
        assign_story_cluster(state, article_id).await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn existing_article_matches_feed_item(
    state: &AppState,
    feed_id: Uuid,
    guid: &str,
    url: &str,
    title: &str,
    author: Option<&str>,
    content: &str,
    published_at: Option<DateTime<Utc>>,
) -> AppResult<Option<Uuid>> {
    let url_hash = hash_url(url);
    let Some(row) = sqlx::query(
        "SELECT id, url, url_hash, title, author, content, published_at
         FROM articles
         WHERE feed_id = $1 AND (url_hash = $3 OR guid = $2)
         ORDER BY (url_hash = $3) DESC
         LIMIT 1",
    )
    .bind(feed_id)
    .bind(guid)
    .bind(&url_hash)
    .fetch_optional(&state.pool)
    .await?
    else {
        return Ok(None);
    };

    let current_published = row.get::<Option<DateTime<Utc>>, _>("published_at");
    let next_published = published_at.or(current_published);
    let unchanged = row.get::<String, _>("url") == url
        && row.get::<String, _>("url_hash") == url_hash
        && row.get::<String, _>("title") == title
        && row.get::<Option<String>, _>("author").as_deref() == author
        && row.get::<String, _>("content") == content
        && current_published == next_published;

    if unchanged {
        Ok(Some(row.get("id")))
    } else {
        Ok(None)
    }
}

#[allow(clippy::too_many_arguments)]
async fn upsert_article(
    state: &AppState,
    feed_id: Uuid,
    _feed_title: &str,
    guid: &str,
    url: &str,
    title: &str,
    author: Option<&str>,
    content: &str,
    extracted_content: &str,
    extraction_error: Option<&str>,
    published_at: Option<DateTime<Utc>>,
) -> AppResult<UpsertedArticle> {
    let url_hash = hash_url(url);
    let canonical_url = canonicalize_url(url);
    let best_content = if extracted_content.trim().is_empty() {
        content
    } else {
        extracted_content
    };
    let content_hash = hash_content(best_content);
    let language = detect_language(title, best_content);
    let quality_score = article_quality_score(best_content, author, extraction_error, published_at);
    if let Some(row) = sqlx::query(
        "SELECT id, url, url_hash, title, author, content, extracted_content, extraction_error,
                published_at, canonical_url, content_hash, language, quality_score
         FROM articles
         WHERE feed_id = $1 AND (url_hash = $3 OR guid = $2)
         ORDER BY (url_hash = $3) DESC
         LIMIT 1",
    )
    .bind(feed_id)
    .bind(guid)
    .bind(&url_hash)
    .fetch_optional(&state.pool)
    .await?
    {
        let id = row.get("id");
        let current_published = row.get::<Option<DateTime<Utc>>, _>("published_at");
        let next_published = published_at.or(current_published);
        let unchanged = row.get::<String, _>("url") == url
            && row.get::<String, _>("url_hash") == url_hash
            && row.get::<String, _>("title") == title
            && row.get::<Option<String>, _>("author").as_deref() == author
            && row.get::<String, _>("content") == content
            && row.get::<String, _>("extracted_content") == extracted_content
            && row.get::<Option<String>, _>("extraction_error").as_deref() == extraction_error
            && current_published == next_published
            && row.get::<String, _>("canonical_url") == canonical_url
            && row.get::<String, _>("content_hash") == content_hash
            && row.get::<Option<String>, _>("language") == language
            && row.get::<i32, _>("quality_score") == quality_score;

        if unchanged {
            return Ok(UpsertedArticle {
                id,
                should_index: false,
            });
        }

        let mut transaction = state.pool.begin().await?;
        lock_story_generation_for_article_write(&mut transaction).await?;
        sqlx::query(
            "UPDATE articles
         SET url = $2,
             url_hash = $3,
             title = $4,
             author = $5,
             content = $6,
             extracted_content = $7,
             extraction_error = $8,
             published_at = COALESCE($9, articles.published_at),
             canonical_url = $10,
             content_hash = $11,
             language = $12,
             quality_score = $13,
             needs_recluster = true,
             updated_at = now()
         WHERE id = $1
         RETURNING id",
        )
        .bind(id)
        .bind(url)
        .bind(&url_hash)
        .bind(title)
        .bind(author)
        .bind(content)
        .bind(extracted_content)
        .bind(extraction_error)
        .bind(published_at)
        .bind(&canonical_url)
        .bind(&content_hash)
        .bind(&language)
        .bind(quality_score)
        .execute(&mut *transaction)
        .await?;
        invalidate_summaries_using_source(&mut transaction, id).await?;
        transaction.commit().await?;
        return Ok(UpsertedArticle {
            id,
            should_index: true,
        });
    }

    let mut transaction = state.pool.begin().await?;
    lock_story_generation_for_article_write(&mut transaction).await?;
    let row = sqlx::query(
        "INSERT INTO articles
          (feed_id, guid, url, url_hash, title, author, content, extracted_content,
           extraction_error, published_at, canonical_url, content_hash, language, quality_score)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
         ON CONFLICT (feed_id, url_hash) DO UPDATE SET
           url = EXCLUDED.url,
           title = EXCLUDED.title,
           author = EXCLUDED.author,
           content = EXCLUDED.content,
           extracted_content = EXCLUDED.extracted_content,
           extraction_error = EXCLUDED.extraction_error,
           published_at = COALESCE(EXCLUDED.published_at, articles.published_at),
           canonical_url = EXCLUDED.canonical_url,
           content_hash = EXCLUDED.content_hash,
           language = EXCLUDED.language,
           quality_score = EXCLUDED.quality_score,
           needs_recluster = true,
           updated_at = now()
         RETURNING id",
    )
    .bind(feed_id)
    .bind(guid)
    .bind(url)
    .bind(url_hash)
    .bind(title)
    .bind(author)
    .bind(content)
    .bind(extracted_content)
    .bind(extraction_error)
    .bind(published_at)
    .bind(canonical_url)
    .bind(content_hash)
    .bind(language)
    .bind(quality_score)
    .fetch_one(&mut *transaction)
    .await?;
    let id = row.get("id");
    invalidate_summaries_using_source(&mut transaction, id).await?;
    transaction.commit().await?;
    Ok(UpsertedArticle {
        id,
        should_index: true,
    })
}

async fn invalidate_summaries_using_source(
    transaction: &mut Transaction<'_, Postgres>,
    article_id: Uuid,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE summaries
         SET status = 'failed',
             run_token = NULL,
             error = '摘要来源正文已更新，请重新生成',
             updated_at = now()
         WHERE status IN ('running', 'done')
           AND (article_id = $1 OR source_article_ids @> ARRAY[$1]::uuid[])",
    )
    .bind(article_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "UPDATE event_digests
         SET status = 'pending', error = '事件证据正文已更新，等待重新核验',
             agent_state = '{\"phase\":\"pending\",\"steps\":[\"source_invalidated\"]}'::jsonb,
             updated_at = now()
         WHERE source_article_ids @> ARRAY[$1]::uuid[]",
    )
    .bind(article_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub async fn queue_summary_for_article(state: &AppState, article_id: Uuid) -> AppResult<bool> {
    let mut transaction = state.pool.begin().await?;
    let article_exists = sqlx::query("SELECT id FROM articles WHERE id = $1 FOR UPDATE")
        .bind(article_id)
        .fetch_optional(&mut *transaction)
        .await?
        .is_some();
    if !article_exists {
        return Err(AppError::NotFound);
    }

    let active_job = sqlx::query(
        "SELECT id FROM worker_jobs
         WHERE kind = 'summarize_article' AND article_id = $1
           AND status IN ('pending', 'running')
         LIMIT 1",
    )
    .bind(article_id)
    .fetch_optional(&mut *transaction)
    .await?
    .is_some();
    if active_job {
        transaction.commit().await?;
        return Ok(false);
    }

    let inserted = sqlx::query(
        "INSERT INTO worker_jobs (kind, article_id)
         VALUES ('summarize_article', $1)
         ON CONFLICT DO NOTHING",
    )
    .bind(article_id)
    .execute(&mut *transaction)
    .await?
    .rows_affected()
        > 0;
    if !inserted {
        transaction.commit().await?;
        return Ok(false);
    }

    sqlx::query(
        "INSERT INTO summaries (article_id, status)
         VALUES ($1, 'pending')
         ON CONFLICT (article_id) DO UPDATE SET
           status = 'pending', run_token = NULL, error = NULL, updated_at = now()",
    )
    .bind(article_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO summary_jobs (article_id, scope, status)
         SELECT $1, 'article', 'pending'
         WHERE NOT EXISTS (
           SELECT 1 FROM summary_jobs
           WHERE article_id = $1 AND status IN ('pending', 'running')
         )
         ON CONFLICT DO NOTHING",
    )
    .bind(article_id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    let _ = state
        .worker_tx
        .try_send(WorkerCommand::SummarizeArticle(article_id));
    Ok(true)
}

pub async fn summarize_article_by_id(
    state: &AppState,
    article_id: Uuid,
    run_token: Uuid,
) -> AppResult<()> {
    let summary_start = sqlx::query(
        "WITH lease AS (
           SELECT 1 FROM worker_jobs
           WHERE kind = 'summarize_article' AND article_id = $1
             AND status = 'running' AND run_token = $2
           FOR UPDATE
         )
         INSERT INTO summaries (article_id, status, run_token)
         SELECT $1, 'running', $2 FROM lease
         ON CONFLICT (article_id) DO UPDATE SET
           status = 'running', run_token = EXCLUDED.run_token, error = NULL, updated_at = now()",
    )
    .bind(article_id)
    .bind(run_token)
    .execute(&state.pool)
    .await?;
    if summary_start.rows_affected() != 1 {
        return Err(AppError::Upstream(
            "摘要任务租约已失效，拒绝启动旧任务".to_string(),
        ));
    }
    sqlx::query(
        "UPDATE summary_jobs
         SET status = 'running', started_at = now(), error = NULL, updated_at = now()
         WHERE article_id = $1 AND status = 'pending'",
    )
    .bind(article_id)
    .execute(&state.pool)
    .await?;

    let _llm_permit = state
        .llm_semaphore
        .acquire()
        .await
        .map_err(|_| AppError::Upstream("摘要并发控制器已关闭".to_string()))?;

    let materials = load_summary_materials(state, article_id)
        .await?
        .into_iter()
        .filter(|material| !material.source.content.trim().is_empty())
        .collect::<Vec<_>>();
    if materials
        .first()
        .is_none_or(|material| material.source.article_id != article_id)
    {
        return Err(AppError::BadRequest("当前文章没有可总结的正文".to_string()));
    }
    let source_article_ids = materials
        .iter()
        .map(|material| material.source.article_id)
        .collect::<Vec<_>>();
    let source_count = materials.len();
    let input_hash = summary_materials_hash(&materials, &state.config);
    let snapshot_scope = if source_count >= 2 {
        "event_snapshot"
    } else {
        "article"
    };
    let snapshot_result = sqlx::query(
        "UPDATE summaries
         SET schema_version = 2, prompt_version = 'grounded-summary-v3',
             scope = $4, source_count = $2, cited_source_count = 0,
             source_article_ids = $3, input_hash = $5, updated_at = now()
         WHERE article_id = $1 AND status = 'running' AND run_token = $6",
    )
    .bind(article_id)
    .bind(source_count as i32)
    .bind(&source_article_ids)
    .bind(snapshot_scope)
    .bind(&input_hash)
    .bind(run_token)
    .execute(&state.pool)
    .await?;
    if snapshot_result.rows_affected() != 1 {
        return Err(AppError::Upstream(
            "摘要任务状态已变化，无法保存来源快照".to_string(),
        ));
    }

    let sources = materials
        .into_iter()
        .map(|material| material.source)
        .collect::<Vec<_>>();
    match summarize_sources(&state.config, &state.http, sources).await {
        Ok(summary) => {
            let latest_materials = load_summary_materials(state, article_id)
                .await?
                .into_iter()
                .filter(|material| !material.source.content.trim().is_empty())
                .collect::<Vec<_>>();
            if summary_materials_hash(&latest_materials, &state.config) != input_hash {
                return Err(AppError::Upstream(
                    "摘要来源在生成期间发生变化，已拒绝写入旧快照".to_string(),
                ));
            }
            let model = summary
                .method
                .starts_with("grounded-")
                .then_some(state.config.llm_model.as_str());
            let result = sqlx::query(
                "UPDATE summaries
                 SET model = $2, summary = $3, bullets = $4, keywords = $5,
                     confidence = $6, confidence_reason = $7, citations = $8,
                     schema_version = $9, prompt_version = $10, method = $11, scope = $12,
                     source_count = $13, source_article_ids = $14, input_hash = $15,
                     grounding_score = $16, corroboration = $17, disagreements = $18,
                     cited_source_count = $19,
                     error = NULL, updated_at = now()
                 WHERE article_id = $1 AND status = 'running' AND input_hash = $15
                   AND run_token = $20",
            )
            .bind(article_id)
            .bind(model)
            .bind(&summary.summary)
            .bind(serde_json::to_value(&summary.bullets).unwrap_or_else(|_| serde_json::json!([])))
            .bind(&summary.keywords)
            .bind(&summary.confidence)
            .bind(&summary.confidence_reason)
            .bind(
                serde_json::to_value(&summary.citations).unwrap_or_else(|_| serde_json::json!([])),
            )
            .bind(summary.schema_version)
            .bind("grounded-summary-v3")
            .bind(&summary.method)
            .bind(&summary.scope)
            .bind(summary.source_count)
            .bind(&source_article_ids)
            .bind(&input_hash)
            .bind(summary.grounding_score)
            .bind(&summary.corroboration)
            .bind(
                serde_json::to_value(&summary.disagreements)
                    .unwrap_or_else(|_| serde_json::json!([])),
            )
            .bind(summary.cited_source_count)
            .bind(run_token)
            .execute(&state.pool)
            .await?;
            if result.rows_affected() != 1 {
                return Err(AppError::Upstream(
                    "摘要任务状态已变化，已拒绝写入过期结果".to_string(),
                ));
            }
        }
        Err(err) => return Err(err),
    }
    Ok(())
}

async fn load_summary_materials(
    state: &AppState,
    article_id: Uuid,
) -> AppResult<Vec<SummaryMaterial>> {
    let row = sqlx::query(
        "SELECT a.id, a.feed_id, a.story_id, a.title, a.url, a.content_hash,
                a.content, a.extracted_content,
                f.title AS source_title, f.site_url, f.feed_url, f.hub_adapter, f.hub_path
         FROM articles a
         JOIN feeds f ON f.id = a.feed_id
         WHERE a.id = $1",
    )
    .bind(article_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    let story_id = row.get::<Option<Uuid>, _>("story_id");
    let primary = summary_material_from_row(&row);
    let Some(story_id) = story_id else {
        return Ok(vec![primary]);
    };
    let rows = sqlx::query(
        "SELECT a.id, a.feed_id, a.title, a.url, a.content_hash,
                a.content, a.extracted_content,
                f.title AS source_title, f.site_url, f.feed_url, f.hub_adapter, f.hub_path
         FROM articles a
         JOIN feeds f ON f.id = a.feed_id
         LEFT JOIN source_feedback sf ON sf.feed_id = f.id
         WHERE a.story_id = $1 AND a.id <> $2
           AND COALESCE(sf.muted, false) = false
         ORDER BY LEAST(100, GREATEST(0, f.quality_score + COALESCE(sf.weight, 0))) DESC,
                  a.quality_score DESC, COALESCE(a.published_at, a.fetched_at) DESC, a.id
         LIMIT 24",
    )
    .bind(story_id)
    .bind(article_id)
    .fetch_all(&state.pool)
    .await?;

    Ok(select_summary_materials(
        primary,
        rows.iter().map(summary_material_from_row),
    ))
}

fn select_summary_materials(
    primary: SummaryMaterial,
    candidates: impl IntoIterator<Item = SummaryMaterial>,
) -> Vec<SummaryMaterial> {
    let mut materials = vec![primary];
    let mut feed_ids = HashSet::from([materials[0].feed_id]);
    let mut content_hashes = HashSet::from([materials[0].content_hash.clone()]);
    let mut url_hashes = HashSet::from([hash_url(&materials[0].source.url)]);
    let mut source_families = HashSet::from([materials[0].source_family.clone()]);
    for material in candidates {
        let url_hash = hash_url(&material.source.url);
        if feed_ids.contains(&material.feed_id)
            || content_hashes.contains(&material.content_hash)
            || url_hashes.contains(&url_hash)
            || source_families.contains(&material.source_family)
            || material.source.content.trim().is_empty()
        {
            continue;
        }
        feed_ids.insert(material.feed_id);
        content_hashes.insert(material.content_hash.clone());
        url_hashes.insert(url_hash);
        source_families.insert(material.source_family.clone());
        materials.push(material);
        if materials.len() >= 5 {
            break;
        }
    }
    materials
}

fn summary_material_from_row(row: &sqlx::postgres::PgRow) -> SummaryMaterial {
    let url = row.get::<String, _>("url");
    let title = row.get::<String, _>("title");
    let content = best_article_text(
        &title,
        &row.get::<String, _>("content"),
        &row.get::<String, _>("extracted_content"),
    );
    SummaryMaterial {
        source: SummarySource {
            article_id: row.get("id"),
            source_title: row.get("source_title"),
            article_title: title,
            url,
            content,
        },
        feed_id: row.get("feed_id"),
        content_hash: row.get("content_hash"),
        source_family: publisher_family(
            row.get::<Option<&str>, _>("site_url"),
            row.get::<&str, _>("feed_url"),
            row.get::<Option<&str>, _>("hub_adapter"),
            row.get::<Option<&str>, _>("hub_path"),
        ),
    }
}

fn summary_material_signature(material: &SummaryMaterial) -> String {
    serde_json::json!([
        material.source.article_id,
        material.feed_id,
        material.source_family,
        material.content_hash,
        material.source.source_title,
        material.source.article_title,
        material.source.url,
    ])
    .to_string()
}

fn summary_materials_hash(materials: &[SummaryMaterial], config: &Config) -> String {
    hash_content(
        &[
            "schema:2".to_string(),
            "pipeline:grounded-summary-v4".to_string(),
            format!("provider:{}", config.llm_base_url.trim_end_matches('/')),
            format!("model:{}", config.llm_model),
            format!("context_budget:{}", config.llm_context_budget_chars),
            format!("output_budget:{}", config.llm_max_output_tokens),
        ]
        .into_iter()
        .chain(materials.iter().map(summary_material_signature))
        .collect::<Vec<_>>()
        .join("\n"),
    )
}

pub async fn reindex_all(state: &AppState) -> AppResult<()> {
    state.search.clear()?;
    let mut last_id = None::<Uuid>;
    loop {
        let rows = sqlx::query(
            "SELECT a.id, f.title AS feed_title, a.title, a.extracted_content,
                    a.content, a.url, a.published_at
             FROM articles a
             JOIN feeds f ON f.id = a.feed_id
             WHERE ($1::uuid IS NULL OR a.id > $1)
             ORDER BY a.id
             LIMIT 2000",
        )
        .bind(last_id)
        .fetch_all(&state.pool)
        .await?;
        if rows.is_empty() {
            break;
        }
        let mut search_documents = Vec::with_capacity(rows.len());
        for row in rows {
            let extracted = row.get::<String, _>("extracted_content");
            let content = if extracted.trim().is_empty() {
                row.get::<String, _>("content")
            } else {
                extracted
            };
            let id = row.get("id");
            last_id = Some(id);
            search_documents.push(SearchDocument {
                id,
                feed_title: row.get("feed_title"),
                title: row.get("title"),
                content: clean_extracted_text(&content),
                url: row.get("url"),
                published_at: row.get("published_at"),
            });
        }
        state.search.index_articles(&search_documents)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_selection_preserves_primary_and_distinct_publishers() {
        let material = |id, family: &str, content: &str| SummaryMaterial {
            source: SummarySource {
                article_id: Uuid::from_u128(id),
                source_title: family.into(),
                article_title: "An event".into(),
                url: format!("https://articles.example/{id}"),
                content: content.into(),
            },
            feed_id: Uuid::from_u128(id),
            content_hash: hash_content(content),
            source_family: family.into(),
        };
        let primary = material(1, "publisher.example", "The requested article");
        let mut same_feed = material(4, "other.example", "Another article on the same feed");
        same_feed.feed_id = primary.feed_id;
        let mut same_url = material(
            7,
            "aggregator.example",
            "A shortened copy of the primary article",
        );
        same_url.source.url = format!("{}?utm_source=feed#summary", primary.source.url);
        let candidates = [
            material(
                2,
                "publisher.example",
                "Same publisher, different feed and article URL",
            ),
            material(3, "copy.example", "The requested article"),
            same_feed,
            same_url,
            material(5, "empty.example", "  "),
            material(6, "independent.example", "An independently written account"),
        ];
        let selected = select_summary_materials(primary.clone(), candidates);
        assert_eq!(
            selected
                .iter()
                .map(|m| m.source.article_id.as_u128())
                .collect::<Vec<_>>(),
            vec![1, 6]
        );
        let selected = select_summary_materials(
            primary,
            (10..20).map(|id| {
                material(
                    id,
                    &format!("publisher{id}.example"),
                    &format!("Report {id}"),
                )
            }),
        );
        assert_eq!(selected.len(), 5);
        assert_eq!(selected[0].source.article_id.as_u128(), 1);
    }

    #[test]
    fn summary_snapshot_changes_when_citation_metadata_changes() {
        let material = SummaryMaterial {
            source: SummarySource {
                article_id: Uuid::from_u128(1),
                source_title: "Publisher".into(),
                article_title: "Original headline".into(),
                url: "https://publisher.example/original".into(),
                content: "Evidence".into(),
            },
            feed_id: Uuid::from_u128(2),
            content_hash: hash_content("Evidence"),
            source_family: "publisher.example".into(),
        };
        let original = summary_material_signature(&material);
        let mut changed = material.clone();
        changed.source.article_title = "Corrected headline".into();
        assert_ne!(summary_material_signature(&changed), original);
        let mut changed = material.clone();
        changed.source.url = "https://publisher.example/correction".into();
        assert_ne!(summary_material_signature(&changed), original);
        let mut changed = material.clone();
        changed.source.source_title = "Renamed publisher".into();
        assert_ne!(summary_material_signature(&changed), original);
        let mut changed = material.clone();
        changed.source_family = "new-publisher.example".into();
        assert_ne!(summary_material_signature(&changed), original);
    }

    #[test]
    fn selects_internal_hub_source_only_for_complete_pairs() {
        assert_eq!(internal_hub_source(None, None).unwrap(), None);
        assert_eq!(
            internal_hub_source(Some("opencli".to_string()), Some("bluesky".to_string())).unwrap(),
            Some(("opencli".to_string(), "bluesky".to_string()))
        );
        assert!(internal_hub_source(Some("opencli".to_string()), None).is_err());
        assert!(internal_hub_source(None, Some("bluesky".to_string())).is_err());
    }

    #[test]
    fn retry_delay_uses_stable_bounded_job_jitter() {
        let job_id = Uuid::from_u128(17);
        let delay = retry_delay_seconds(job_id, 1);
        assert_eq!(delay, retry_delay_seconds(job_id, 1));
        assert!((24..=30).contains(&delay));

        let capped = retry_delay_seconds(job_id, i32::MAX);
        assert!((2_880..=3_600).contains(&capped));
    }

    #[test]
    fn feed_failures_defer_to_the_feed_schedule_without_hot_retries() {
        let error = AppError::Upstream("Feed 请求失败: 403 Forbidden".to_string());
        assert!(feed_failure_uses_schedule(&error));
        assert!(job_failure_is_terminal(
            &WorkerCommand::FetchFeed(Uuid::nil()),
            1,
            5,
            &error,
        ));
        assert!(!job_failure_is_terminal(
            &WorkerCommand::SummarizeArticle(Uuid::nil()),
            1,
            5,
            &error,
        ));

        let internal_error = AppError::Anyhow(anyhow::anyhow!("index unavailable"));
        assert!(!feed_failure_uses_schedule(&internal_error));
        assert!(!job_failure_is_terminal(
            &WorkerCommand::FetchFeed(Uuid::nil()),
            1,
            5,
            &internal_error,
        ));
        assert!(job_failure_is_terminal(
            &WorkerCommand::FetchFeed(Uuid::nil()),
            5,
            5,
            &internal_error,
        ));
    }
}
