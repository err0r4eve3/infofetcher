use std::{
    collections::{HashMap, HashSet},
    sync::atomic::{AtomicBool, Ordering},
};

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::Row;
use tokio::time;
use uuid::Uuid;

use crate::{
    config::Config,
    error::{AppError, AppResult},
    extractor::best_article_text,
    feeds::hash_url,
    intelligence::{hash_content, publisher_family},
    llm::{
        EVENT_TITLE_TRANSLATION_PROMPT_VERSION, GeneratedSummary, SummarySource,
        summarize_event_sources, translate_event_title_with_llm,
    },
    state::AppState,
    translation,
};

const EVENT_AGENT_LOCK_KEY: i64 = 0x5253_5345_564e_5401;
const PROMPT_VERSION: &str = "event-wire-agent-v10";
const MIN_WIRE_SOURCE_QUALITY: i32 = 70;
static DEDICATED_TITLE_TRANSLATION_DISABLED: AtomicBool = AtomicBool::new(false);

struct AgentRun {
    id: Uuid,
    story_id: Uuid,
    run_token: Uuid,
    attempts: i32,
    max_attempts: i32,
}

struct EventMaterial {
    source: SummarySource,
    feed_id: Uuid,
    content_hash: String,
    source_family: String,
}

struct EventContext {
    topic: String,
    revision_type: String,
    revision_id: Option<Uuid>,
    focus_article_id: Option<Uuid>,
    story_updated_at: DateTime<Utc>,
    published_at: DateTime<Utc>,
}

struct Verification {
    status: &'static str,
    label: &'static str,
    score: i32,
    reason: String,
}

pub fn spawn(state: AppState) {
    if !state.config.event_agent_enabled {
        tracing::info!("event wire agent disabled");
        return;
    }
    tokio::spawn(async move {
        loop {
            let mut connection = match state.pool.acquire().await {
                Ok(connection) => connection,
                Err(error) => {
                    tracing::warn!(%error, "event agent leader connection failed");
                    time::sleep(time::Duration::from_secs(5)).await;
                    continue;
                }
            };
            let acquired = sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock($1)")
                .bind(EVENT_AGENT_LOCK_KEY)
                .fetch_one(&mut *connection)
                .await
                .unwrap_or(false);
            if !acquired {
                time::sleep(time::Duration::from_secs(10)).await;
                continue;
            }
            tracing::info!("event wire agent leader lock acquired");
            if let Err(error) = recover_stale_runs(&state).await {
                tracing::warn!(%error, "event agent stale-run recovery failed");
            }
            loop {
                if sqlx::query("SELECT 1")
                    .fetch_one(&mut *connection)
                    .await
                    .is_err()
                {
                    tracing::warn!("event agent leader connection lost");
                    break;
                }
                let sleep_for = match run_once(&state).await {
                    Ok(true) => time::Duration::from_secs(1),
                    Ok(false) => state.config.event_agent_interval,
                    Err(error) => {
                        tracing::warn!(%error, "event agent turn failed");
                        state.config.event_agent_interval
                    }
                };
                time::sleep(sleep_for).await;
            }
        }
    });
}

pub async fn run_once(state: &AppState) -> AppResult<bool> {
    if !state.config.event_agent_enabled {
        return Ok(false);
    }
    enqueue_due_stories(state).await?;
    let Some(run) = claim_next_run(state).await? else {
        return Ok(false);
    };
    match execute_run(state, &run).await {
        Ok(()) => complete_run(state, &run).await?,
        Err(error) => {
            fail_run(state, &run, &error).await?;
            return Err(error);
        }
    }
    Ok(true)
}

pub async fn queue_story(state: &AppState, story_id: Uuid) -> AppResult<bool> {
    let exists =
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM story_clusters WHERE id = $1)")
            .bind(story_id)
            .fetch_one(&state.pool)
            .await?;
    if !exists {
        return Err(AppError::NotFound);
    }
    let inserted = sqlx::query(
        "INSERT INTO event_agent_runs (story_id)
         VALUES ($1)
         ON CONFLICT DO NOTHING",
    )
    .bind(story_id)
    .execute(&state.pool)
    .await?
    .rows_affected()
        > 0;
    if inserted {
        sqlx::query(
            "INSERT INTO event_digests (story_id, status)
             VALUES ($1, 'pending')
             ON CONFLICT (story_id) DO UPDATE SET
               status = 'pending', error = NULL, updated_at = now()",
        )
        .bind(story_id)
        .execute(&state.pool)
        .await?;
    }
    Ok(inserted)
}

async fn recover_stale_runs(state: &AppState) -> AppResult<()> {
    sqlx::query(
        "UPDATE event_agent_runs
         SET status = 'pending', run_token = NULL, locked_at = NULL,
             phase = 'pending', error = COALESCE(error, '进程重启后恢复'),
             available_at = now(), updated_at = now()
         WHERE status = 'running'",
    )
    .execute(&state.pool)
    .await?;
    sqlx::query(
        "UPDATE event_digests
         SET status = 'pending', agent_state = '{\"phase\":\"pending\",\"steps\":[\"recovered\"]}'::jsonb,
             updated_at = now()
         WHERE status = 'running'",
    )
    .execute(&state.pool)
    .await?;
    Ok(())
}

async fn enqueue_due_stories(state: &AppState) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO event_agent_runs (story_id)
         SELECT sc.id
         FROM story_clusters sc
         LEFT JOIN event_digests digest ON digest.story_id = sc.id
         WHERE sc.source_count >= 2
           AND sc.quality_score >= 55
           AND EXISTS (
             SELECT 1
             FROM articles quality_article
             JOIN feeds quality_feed ON quality_feed.id = quality_article.feed_id
             WHERE quality_article.story_id = sc.id
               AND quality_feed.quality_score >= $4
           )
           AND COALESCE(sc.last_published_at, sc.updated_at) >=
               now() - ($1 || ' hours')::interval
           AND (digest.story_id IS NULL
                OR digest.prompt_version <> $3
                OR (
                  (digest.status = 'pending'
                   AND digest.updated_at <= now() - ($5 || ' seconds')::interval)
                  OR (digest.status <> 'pending'
                      AND digest.updated_at < sc.updated_at
                      AND digest.updated_at <= now() - ($5 || ' seconds')::interval)
                  OR EXISTS (
                    SELECT 1
                    FROM event_revisions urgent_revision
                    WHERE urgent_revision.story_id = sc.id
                      AND urgent_revision.created_at >
                          COALESCE(digest.completed_at, digest.updated_at)
                      AND urgent_revision.revision_type IN ('CORRECT', 'CONTRADICT')
                  )
                ))
           AND NOT EXISTS (
             SELECT 1 FROM event_agent_runs run
             WHERE run.story_id = sc.id AND run.status IN ('pending', 'running')
           )
         ORDER BY COALESCE(sc.last_published_at, sc.updated_at) DESC,
                  sc.source_count DESC
         LIMIT GREATEST(
           $2 - (
             SELECT COUNT(*) FROM event_agent_runs
             WHERE status IN ('pending', 'running')
           ),
           0
         )
         ON CONFLICT DO NOTHING",
    )
    .bind(state.config.event_agent_lookback_hours)
    .bind(state.config.event_agent_max_pending as i64)
    .bind(PROMPT_VERSION)
    .bind(MIN_WIRE_SOURCE_QUALITY)
    .bind(state.config.event_agent_update_debounce.as_secs() as i64)
    .execute(&state.pool)
    .await?;
    sqlx::query(
        "INSERT INTO event_digests (story_id, status)
         SELECT DISTINCT story_id, 'pending'
         FROM event_agent_runs
         WHERE status = 'pending'
         ON CONFLICT (story_id) DO NOTHING",
    )
    .execute(&state.pool)
    .await?;
    Ok(())
}

async fn claim_next_run(state: &AppState) -> AppResult<Option<AgentRun>> {
    let mut transaction = state.pool.begin().await?;
    let row = sqlx::query(
        "WITH candidate AS (
           SELECT id
           FROM event_agent_runs
           WHERE status = 'pending' AND available_at <= now()
           ORDER BY available_at, created_at
           FOR UPDATE SKIP LOCKED
           LIMIT 1
         )
         UPDATE event_agent_runs run
         SET status = 'running', attempts = attempts + 1,
             run_token = gen_random_uuid(), locked_at = now(),
             phase = 'collecting', error = NULL, updated_at = now()
         FROM candidate
         WHERE run.id = candidate.id
         RETURNING run.id, run.story_id, run.run_token, run.attempts, run.max_attempts",
    )
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let run = AgentRun {
        id: row.get("id"),
        story_id: row.get("story_id"),
        run_token: row.get("run_token"),
        attempts: row.get("attempts"),
        max_attempts: row.get("max_attempts"),
    };
    let digest = sqlx::query(
        "UPDATE event_digests
         SET status = 'running', error = NULL,
             agent_state = '{\"phase\":\"collecting\",\"steps\":[\"read_event\"]}'::jsonb,
             updated_at = now()
         WHERE story_id = $1",
    )
    .bind(run.story_id)
    .execute(&mut *transaction)
    .await?;
    if digest.rows_affected() != 1 {
        return Err(AppError::Upstream(
            "事件 Agent 无法取得摘要状态租约".to_string(),
        ));
    }
    transaction.commit().await?;
    Ok(Some(run))
}

async fn execute_run(state: &AppState, run: &AgentRun) -> AppResult<()> {
    update_phase(state, run, "collecting", &["read_event", "select_evidence"]).await?;
    let context = load_event_context(state, run.story_id).await?;
    let materials = load_event_materials(state, run.story_id, &context).await?;
    let effective_config = event_llm_config(&state.config);
    let source_hash = event_materials_hash(&materials, &effective_config, &state.config, &context);
    let source_article_ids = materials
        .iter()
        .map(|material| material.source.article_id)
        .collect::<Vec<_>>();
    let source_titles = materials
        .iter()
        .map(|material| material.source.source_title.clone())
        .collect::<Vec<_>>();
    let snapshot = sqlx::query(
        "UPDATE event_digests
         SET status = 'running', topic = $2, revision_type = $3,
             source_hash = $4, source_article_ids = $5, source_titles = $6,
             source_count = $7, cited_source_count = 0,
             agent_state = $8, error = NULL, updated_at = now()
         WHERE story_id = $1 AND status = 'running'
           AND EXISTS (
             SELECT 1 FROM event_agent_runs run
             WHERE run.id = $9 AND run.run_token = $10 AND run.status = 'running'
           )",
    )
    .bind(run.story_id)
    .bind(&context.topic)
    .bind(&context.revision_type)
    .bind(&source_hash)
    .bind(&source_article_ids)
    .bind(&source_titles)
    .bind(materials.len() as i32)
    .bind(json!({"phase":"synthesizing","steps":["read_event","select_evidence"]}))
    .bind(run.id)
    .bind(run.run_token)
    .execute(&state.pool)
    .await?;
    if snapshot.rows_affected() != 1 {
        return Err(AppError::Upstream(
            "事件 Agent 状态或来源在取样期间发生变化".to_string(),
        ));
    }
    if materials.len() < 2 {
        skip_digest(
            state,
            run,
            &source_hash,
            "去重后缺少两个可区分的来源，未发布快讯",
        )
        .await?;
        return Ok(());
    }

    update_phase(
        state,
        run,
        "synthesizing",
        &["read_event", "select_evidence", "draft_claims"],
    )
    .await?;
    let wire_topic = wire_focus_title(&context.topic);
    let focus_lead = if contains_cjk(&wire_topic) {
        wire_topic
    } else {
        translate_event_focus(state, &effective_config, &wire_topic).await?
    };
    let summary = {
        let _permit = state
            .llm_semaphore
            .acquire()
            .await
            .map_err(|_| AppError::Upstream("事件 Agent 并发控制器已关闭".to_string()))?;
        summarize_event_sources(
            &effective_config,
            &state.http,
            &context.topic,
            &focus_lead,
            materials
                .iter()
                .map(|material| material.source.clone())
                .collect(),
        )
        .await?
    };

    if !summary.method.starts_with("grounded-") {
        skip_digest(
            state,
            run,
            &source_hash,
            "生成式核验未通过，拒绝把抽取片段作为快讯发布",
        )
        .await?;
        return Ok(());
    }

    update_phase(
        state,
        run,
        "verifying",
        &[
            "read_event",
            "select_evidence",
            "draft_claims",
            "verify_claims",
        ],
    )
    .await?;
    let latest_context = load_event_context(state, run.story_id).await?;
    let latest_materials = load_event_materials(state, run.story_id, &latest_context).await?;
    if event_materials_hash(
        &latest_materials,
        &effective_config,
        &state.config,
        &latest_context,
    ) != source_hash
    {
        return Err(AppError::Upstream(
            "事件证据在 Agent 运行期间发生变化，拒绝发布旧结果".to_string(),
        ));
    }
    let core_cited_source_count = core_cited_source_count(&summary, &materials);
    if core_cited_source_count == 0 {
        skip_digest(
            state,
            run,
            &source_hash,
            "核心句没有通过核验的直接证据，未发布快讯",
        )
        .await?;
        return Ok(());
    }
    let verification =
        classify_verification(&summary, &context.revision_type, core_cited_source_count);
    let message = compact_message(
        &summary.summary,
        &summary.bullets,
        &summary.disagreements,
        verification.status,
    );
    if verification.status == "disputed" && !message.contains("；来源说法不一：") {
        skip_digest(
            state,
            run,
            &source_hash,
            "分歧说明无法在快讯长度内完整呈现，未发布快讯",
        )
        .await?;
        return Ok(());
    }
    if let Some(reason) = wire_message_rejection_reason(&message) {
        skip_digest(state, run, &source_hash, reason).await?;
        return Ok(());
    }
    let citations =
        serde_json::to_value(&summary.citations).map_err(|error| AppError::Anyhow(error.into()))?;
    let disagreements = serde_json::to_value(&summary.disagreements)
        .map_err(|error| AppError::Anyhow(error.into()))?;
    let model = summary
        .method
        .starts_with("grounded-")
        .then_some(effective_config.llm_model.as_str());
    let published = sqlx::query(
        "UPDATE event_digests digest
         SET status = 'done', message = $3,
             verification_status = $4, verification_label = $5,
             verification_score = $6, verification_reason = $7,
             model = $8, prompt_version = $9,
             cited_source_count = $10, citations = $11, disagreements = $12,
             agent_state = $13, published_at = $14,
             error = NULL, completed_at = now(), updated_at = now()
         WHERE digest.story_id = $1 AND digest.source_hash = $2
           AND digest.status = 'running'
           AND EXISTS (
             SELECT 1 FROM event_agent_runs run
             WHERE run.id = $15 AND run.run_token = $16 AND run.status = 'running'
           )",
    )
    .bind(run.story_id)
    .bind(&source_hash)
    .bind(&message)
    .bind(verification.status)
    .bind(verification.label)
    .bind(verification.score)
    .bind(&verification.reason)
    .bind(model)
    .bind(PROMPT_VERSION)
    .bind(core_cited_source_count)
    .bind(citations)
    .bind(disagreements)
    .bind(json!({
        "phase":"complete",
        "completion_signal":"publish_digest",
        "steps":["read_event","select_evidence","draft_claims","verify_claims","publish_digest"]
    }))
    .bind(context.published_at)
    .bind(run.id)
    .bind(run.run_token)
    .execute(&state.pool)
    .await?;
    if published.rows_affected() != 1 {
        return Err(AppError::Upstream(
            "事件 Agent 租约或来源快照已失效，拒绝发布".to_string(),
        ));
    }
    Ok(())
}

async fn translate_event_focus(
    state: &AppState,
    effective_config: &Config,
    wire_topic: &str,
) -> AppResult<String> {
    let dedicated_configured = !state.config.translation_api_key.trim().is_empty();
    let main_llm_configured = !state.config.llm_api_key.trim().is_empty();
    if dedicated_configured && !DEDICATED_TITLE_TRANSLATION_DISABLED.load(Ordering::Relaxed) {
        let result = {
            let _permit = state
                .translation_semaphore
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| AppError::Upstream("事件标题翻译并发控制器已关闭".to_string()))?;
            translation::translate_title(&state.config, &state.http, wire_topic).await
        };
        match result {
            Ok(translation) => return Ok(translation),
            Err(error) if !main_llm_configured => return Err(error),
            Err(error) => {
                if matches!(error, AppError::PermanentUpstream(_)) {
                    DEDICATED_TITLE_TRANSLATION_DISABLED.store(true, Ordering::Relaxed);
                }
                tracing::warn!(
                    %error,
                    disabled = DEDICATED_TITLE_TRANSLATION_DISABLED.load(Ordering::Relaxed),
                    "专用标题翻译不可用，切换主摘要模型"
                );
            }
        }
    }

    let _permit = state
        .llm_semaphore
        .acquire()
        .await
        .map_err(|_| AppError::Upstream("事件 Agent 并发控制器已关闭".to_string()))?;
    translate_event_title_with_llm(effective_config, &state.http, wire_topic).await
}

async fn skip_digest(
    state: &AppState,
    run: &AgentRun,
    source_hash: &str,
    reason: &str,
) -> AppResult<()> {
    let skipped = sqlx::query(
        "UPDATE event_digests
         SET status = 'skipped', message = NULL,
             verification_status = NULL, verification_label = NULL,
             verification_score = NULL, verification_reason = NULL,
             error = $3, prompt_version = $4,
             completed_at = now(), updated_at = now(),
             agent_state = jsonb_build_object(
               'phase', 'skipped',
               'completion_signal', 'skip_digest',
               'reason', $3,
               'steps', agent_state->'steps'
             )
         WHERE story_id = $1 AND source_hash = $2 AND status = 'running'
           AND EXISTS (
             SELECT 1 FROM event_agent_runs active
             WHERE active.id = $5 AND active.run_token = $6 AND active.status = 'running'
           )",
    )
    .bind(run.story_id)
    .bind(source_hash)
    .bind(reason)
    .bind(PROMPT_VERSION)
    .bind(run.id)
    .bind(run.run_token)
    .execute(&state.pool)
    .await?;
    if skipped.rows_affected() != 1 {
        return Err(AppError::Upstream(
            "事件 Agent 租约或来源快照已失效，拒绝写入跳过状态".to_string(),
        ));
    }
    tracing::info!(story_id = %run.story_id, %reason, "event wire digest skipped");
    Ok(())
}

async fn complete_run(state: &AppState, run: &AgentRun) -> AppResult<()> {
    let updated = sqlx::query(
        "UPDATE event_agent_runs
         SET status = 'done', phase = 'complete', run_token = NULL,
             locked_at = NULL, error = NULL, completed_at = now(), updated_at = now()
         WHERE id = $1 AND run_token = $2 AND status = 'running'",
    )
    .bind(run.id)
    .bind(run.run_token)
    .execute(&state.pool)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Upstream("事件 Agent 完成租约已失效".to_string()));
    }
    Ok(())
}

async fn fail_run(state: &AppState, run: &AgentRun, error: &AppError) -> AppResult<()> {
    let terminal = run.attempts >= run.max_attempts
        || matches!(
            error,
            AppError::BadRequest(_) | AppError::PermanentUpstream(_) | AppError::NotFound
        );
    let delay = retry_delay_seconds(run.id, run.attempts);
    let mut transaction = state.pool.begin().await?;
    let owned = sqlx::query(
        "SELECT id FROM event_agent_runs
         WHERE id = $1 AND run_token = $2 AND status = 'running'
         FOR UPDATE",
    )
    .bind(run.id)
    .bind(run.run_token)
    .fetch_optional(&mut *transaction)
    .await?
    .is_some();
    if !owned {
        return Err(AppError::Upstream(
            "事件 Agent 失败租约已被其他实例接管".to_string(),
        ));
    }
    sqlx::query(
        "UPDATE event_digests
         SET status = $2, error = $3,
             prompt_version = CASE WHEN $2 = 'failed' THEN $4 ELSE prompt_version END,
             agent_state = jsonb_build_object('phase', $2, 'steps', agent_state->'steps'),
             updated_at = now()
         WHERE story_id = $1 AND status = 'running'",
    )
    .bind(run.story_id)
    .bind(if terminal { "failed" } else { "pending" })
    .bind(error.to_string())
    .bind(PROMPT_VERSION)
    .execute(&mut *transaction)
    .await?;
    let updated = sqlx::query(
        "UPDATE event_agent_runs
         SET status = $3, phase = $3, run_token = NULL, locked_at = NULL,
             error = $4,
             available_at = CASE WHEN $3 = 'pending'
               THEN now() + ($5 || ' seconds')::interval ELSE available_at END,
             completed_at = CASE WHEN $3 = 'failed' THEN now() ELSE NULL END,
             updated_at = now()
         WHERE id = $1 AND run_token = $2 AND status = 'running'",
    )
    .bind(run.id)
    .bind(run.run_token)
    .bind(if terminal { "failed" } else { "pending" })
    .bind(error.to_string())
    .bind(delay)
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Upstream(
            "事件 Agent 失败租约在提交前失效".to_string(),
        ));
    }
    transaction.commit().await?;
    Ok(())
}

async fn update_phase(
    state: &AppState,
    run: &AgentRun,
    phase: &str,
    steps: &[&str],
) -> AppResult<()> {
    let updated = sqlx::query(
        "UPDATE event_agent_runs SET phase = $3, updated_at = now()
         WHERE id = $1 AND run_token = $2 AND status = 'running'",
    )
    .bind(run.id)
    .bind(run.run_token)
    .bind(phase)
    .execute(&state.pool)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Upstream("事件 Agent 阶段租约已失效".to_string()));
    }
    let digest = sqlx::query(
        "UPDATE event_digests SET agent_state = $2, updated_at = now()
         WHERE story_id = $1 AND status = 'running'",
    )
    .bind(run.story_id)
    .bind(json!({"phase":phase,"steps":steps}))
    .execute(&state.pool)
    .await?;
    if digest.rows_affected() != 1 {
        return Err(AppError::Upstream(
            "事件 Agent 摘要状态已被新的来源版本取代".to_string(),
        ));
    }
    Ok(())
}

async fn load_event_context(state: &AppState, story_id: Uuid) -> AppResult<EventContext> {
    let row = sqlx::query(
        "SELECT COALESCE(NULLIF(revision.title, ''), NULLIF(fallback.title, ''),
                         sc.representative_title) AS topic,
                COALESCE(revision.occurred_at, fallback.observed_at,
                         sc.last_published_at, sc.updated_at) AS published_at,
                COALESCE(revision.revision_type, 'ADD') AS revision_type,
                revision.id AS revision_id,
                COALESCE(revision.article_id, fallback.id) AS focus_article_id,
                sc.updated_at AS story_updated_at
         FROM story_clusters sc
         LEFT JOIN LATERAL (
           SELECT er.id, a.title,
                  er.revision_type, er.article_id, er.occurred_at
           FROM event_revisions er
           JOIN articles a
             ON a.id = er.article_id AND a.story_id = sc.id
           JOIN feeds f ON f.id = a.feed_id
           LEFT JOIN source_feedback sf ON sf.feed_id = a.feed_id
           WHERE er.story_id = sc.id
             AND COALESCE(sf.muted, false) = false
           ORDER BY (er.revision_type IN ('CORRECT', 'CONTRADICT')) DESC,
                    (char_length(a.title) <= 160
                      AND a.title !~ '(…|[.]{3})$') DESC,
                    f.quality_score DESC,
                    a.quality_score DESC,
                    er.occurred_at DESC, er.created_at DESC
           LIMIT 1
         ) revision ON true
         LEFT JOIN LATERAL (
           SELECT a.id, a.title, COALESCE(a.published_at, a.fetched_at) AS observed_at
           FROM articles a
           LEFT JOIN source_feedback sf ON sf.feed_id = a.feed_id
           WHERE a.story_id = sc.id AND COALESCE(sf.muted, false) = false
           ORDER BY COALESCE(a.published_at, a.fetched_at) DESC,
                    a.quality_score DESC, a.id
           LIMIT 1
         ) fallback ON revision.id IS NULL
         WHERE sc.id = $1",
    )
    .bind(story_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(EventContext {
        topic: row.get("topic"),
        revision_type: row.get("revision_type"),
        revision_id: row.get("revision_id"),
        focus_article_id: row.get("focus_article_id"),
        story_updated_at: row.get("story_updated_at"),
        published_at: row.get("published_at"),
    })
}

async fn load_event_materials(
    state: &AppState,
    story_id: Uuid,
    context: &EventContext,
) -> AppResult<Vec<EventMaterial>> {
    let rows = sqlx::query(
        "SELECT a.id, a.feed_id, a.title, a.url, a.content_hash,
                a.content, a.extracted_content, f.title AS source_title,
                f.site_url, f.feed_url, f.hub_adapter, f.hub_path
         FROM articles a
         JOIN feeds f ON f.id = a.feed_id
         JOIN story_clusters sc ON sc.id = a.story_id
         LEFT JOIN source_feedback sf ON sf.feed_id = f.id
         WHERE a.story_id = $1 AND COALESCE(sf.muted, false) = false
         ORDER BY COALESCE(a.id = $2, false) DESC,
                  (a.title = $3) DESC,
                  LEAST(100, GREATEST(0, f.quality_score + COALESCE(sf.weight, 0))) DESC,
                  a.quality_score DESC,
                  COALESCE(a.published_at, a.fetched_at) DESC,
                  a.id
         LIMIT 30",
    )
    .bind(story_id)
    .bind(context.focus_article_id)
    .bind(&context.topic)
    .fetch_all(&state.pool)
    .await?;
    let mut materials = Vec::new();
    let mut feed_ids = HashSet::new();
    let mut content_hashes = HashSet::new();
    let mut url_hashes = HashSet::new();
    let mut source_families = HashSet::new();
    for row in rows {
        let feed_id = row.get::<Uuid, _>("feed_id");
        let content_hash = row.get::<String, _>("content_hash");
        let url = row.get::<String, _>("url");
        let url_hash = hash_url(&url);
        let site_url = row.get::<Option<String>, _>("site_url");
        let feed_url = row.get::<String, _>("feed_url");
        let hub_adapter = row.get::<Option<String>, _>("hub_adapter");
        let hub_path = row.get::<Option<String>, _>("hub_path");
        let source_family = publisher_family(
            site_url.as_deref(),
            &feed_url,
            hub_adapter.as_deref(),
            hub_path.as_deref(),
        );
        if feed_ids.contains(&feed_id)
            || content_hashes.contains(&content_hash)
            || url_hashes.contains(&url_hash)
            || source_families.contains(&source_family)
        {
            continue;
        }
        let title = row.get::<String, _>("title");
        let content = best_article_text(
            &title,
            &row.get::<String, _>("content"),
            &row.get::<String, _>("extracted_content"),
        );
        if content.trim().is_empty() {
            continue;
        }
        feed_ids.insert(feed_id);
        content_hashes.insert(content_hash.clone());
        url_hashes.insert(url_hash);
        source_families.insert(source_family.clone());
        materials.push(EventMaterial {
            source: SummarySource {
                article_id: row.get("id"),
                source_title: row.get("source_title"),
                article_title: title,
                url,
                content,
            },
            feed_id,
            content_hash,
            source_family,
        });
        if materials.len() >= 5 {
            break;
        }
    }
    Ok(materials)
}

fn event_llm_config(config: &Config) -> Config {
    let mut effective = config.clone();
    if effective.llm_api_key.trim().is_empty() && !effective.translation_api_key.trim().is_empty() {
        effective.llm_base_url = effective.translation_base_url.clone();
        effective.llm_api_key = effective.translation_api_key.clone();
        effective.llm_model = effective.translation_model.clone();
        effective.llm_timeout = effective.translation_timeout;
        effective.llm_max_output_tokens = effective
            .llm_max_output_tokens
            .min(effective.translation_max_output_tokens);
    }
    effective
}

fn event_materials_hash(
    materials: &[EventMaterial],
    llm_config: &Config,
    translation_config: &Config,
    context: &EventContext,
) -> String {
    let (translation_route, translation_base_url, translation_model, translation_prompt) =
        if !translation_config.translation_api_key.trim().is_empty() {
            (
                "dedicated",
                translation_config.translation_base_url.as_str(),
                translation_config.translation_model.as_str(),
                translation::PROMPT_VERSION,
            )
        } else {
            (
                "event_llm",
                llm_config.llm_base_url.as_str(),
                llm_config.llm_model.as_str(),
                EVENT_TITLE_TRANSLATION_PROMPT_VERSION,
            )
        };
    hash_content(
        &[
            format!("prompt:{PROMPT_VERSION}"),
            format!("focus:{}", collapse_whitespace(&context.topic)),
            format!(
                "focus_article:{}",
                context
                    .focus_article_id
                    .map(|article_id| article_id.to_string())
                    .unwrap_or_default()
            ),
            format!(
                "revision_id:{}",
                context
                    .revision_id
                    .map(|revision_id| revision_id.to_string())
                    .unwrap_or_default()
            ),
            format!("revision_type:{}", context.revision_type),
            format!("story_updated_at:{}", context.story_updated_at),
            format!("published_at:{}", context.published_at),
            format!("provider:{}", llm_config.llm_base_url.trim_end_matches('/')),
            format!("model:{}", llm_config.llm_model),
            format!("translation_route:{translation_route}"),
            format!(
                "translation_provider:{}",
                translation_base_url.trim_end_matches('/')
            ),
            format!("translation_model:{translation_model}"),
            format!("translation_prompt:{translation_prompt}"),
        ]
        .into_iter()
        .chain(materials.iter().map(|material| {
            format!(
                "{}:{}:{}:{}:{}:{}:{}",
                material.source.article_id,
                material.feed_id,
                material.source_family,
                material.content_hash,
                collapse_whitespace(&material.source.source_title),
                collapse_whitespace(&material.source.article_title),
                material.source.url
            )
        }))
        .collect::<Vec<_>>()
        .join("\n"),
    )
}

fn contains_cjk(value: &str) -> bool {
    value
        .chars()
        .any(|character| ('\u{4e00}'..='\u{9fff}').contains(&character))
}

fn wire_focus_title(value: &str) -> String {
    let mut title = collapse_whitespace(value);
    for suffix in [
        " – as it happened",
        " — as it happened",
        " - as it happened",
    ] {
        if title.to_ascii_lowercase().ends_with(suffix) {
            title.truncate(title.len() - suffix.len());
            title = title.trim_end().to_string();
            break;
        }
    }
    let Some((head, body)) = title.split_once(':') else {
        return title;
    };
    let head_lower = head.to_ascii_lowercase();
    if ["live", "live updates", "live blog"].contains(&head_lower.as_str()) {
        return body.trim_start().to_string();
    }
    title
}

fn core_cited_source_count(summary: &GeneratedSummary, materials: &[EventMaterial]) -> i32 {
    let material_families = materials
        .iter()
        .map(|material| (material.source.article_id, material.source_family.clone()))
        .collect::<HashMap<_, _>>();
    summary
        .citations
        .iter()
        .filter(|citation| citation.verified && citation.claim_id == "summary")
        .map(|citation| {
            material_families
                .get(&citation.article_id)
                .cloned()
                .or_else(|| source_family_from_url(&citation.url))
                .unwrap_or_else(|| citation.article_id.to_string())
        })
        .collect::<HashSet<_>>()
        .len() as i32
}

fn classify_verification(
    summary: &GeneratedSummary,
    revision_type: &str,
    core_cited_source_count: i32,
) -> Verification {
    let grounding = summary.grounding_score.unwrap_or(35).clamp(0, 100);
    if revision_type == "CORRECT" {
        return Verification {
            status: "corrected",
            label: "已更正",
            score: grounding.min(85),
            reason: format!(
                "事件包含更正记录；{} 个来源支撑当前版本。",
                core_cited_source_count
            ),
        };
    }
    if summary.corroboration == "divergent" || !summary.disagreements.is_empty() {
        return Verification {
            status: "disputed",
            label: "存在分歧",
            score: grounding.min(70),
            reason: format!(
                "来源对部分主张存在冲突；已保留 {} 组分歧。",
                summary.disagreements.len().max(1)
            ),
        };
    }
    if !summary.method.starts_with("grounded-") || core_cited_source_count == 0 {
        return Verification {
            status: "insufficient",
            label: "证据不足",
            score: grounding.min(40),
            reason: "仅能抽取原文，未完成生成式多源核验。".to_string(),
        };
    }
    if summary.corroboration == "multi" && core_cited_source_count >= 2 {
        return Verification {
            status: "corroborated",
            label: "多源一致",
            score: grounding.max(80),
            reason: format!(
                "{} 个可区分来源直接支撑核心主张；未发现材料内冲突。",
                core_cited_source_count
            ),
        };
    }
    Verification {
        status: "developing",
        label: "持续核验",
        score: grounding.min(79),
        reason: format!(
            "当前有 {} 个来源进入分析，{} 个来源实际支撑已发布主张。",
            summary.source_count, core_cited_source_count
        ),
    }
}

fn compact_message(
    summary: &str,
    bullets: &[String],
    disagreements: &[String],
    verification_status: &str,
) -> String {
    let mut message = collapse_whitespace(summary)
        .trim_matches(['；', '。', ' '])
        .to_string();
    if verification_status != "disputed"
        && let Some(point) = bullets
            .iter()
            .map(|value| {
                collapse_whitespace(value)
                    .trim_matches(['；', '。', ' '])
                    .to_string()
            })
            .find(|value| !value.is_empty() && !is_redundant_point(&message, value))
    {
        let candidate = format!("{message}；{}", point.trim_matches(['；', '。', ' ']));
        if candidate.chars().count() <= 220 {
            message = candidate;
        }
    }
    if verification_status == "disputed"
        && let Some(disagreement) = disagreements
            .iter()
            .map(|value| collapse_whitespace(value))
            .find(|value| !value.is_empty() && !message.contains(value))
    {
        let disagreement = disagreement.trim_matches(['；', '。', ' ']);
        let candidate = format!("{message}；来源说法不一：{disagreement}");
        if candidate.chars().count() <= 220 {
            message = candidate;
        }
    }
    clip_chars(message.trim_matches(['；', '。', ' ']), 220)
}

fn message_key(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_alphanumeric())
        .collect()
}

fn is_redundant_point(summary: &str, point: &str) -> bool {
    let summary_key = message_key(summary);
    let point_key = message_key(point);
    if summary_key.contains(&point_key) || point_key.contains(&summary_key) {
        return true;
    }
    let summary_terms = message_terms(summary);
    let point_terms = message_terms(point);
    if summary_terms.is_empty() || point_terms.is_empty() {
        return false;
    }
    let intersection = summary_terms.intersection(&point_terms).count();
    let containment = intersection as f32 / summary_terms.len().min(point_terms.len()) as f32;
    intersection >= 4 && containment >= 0.45
}

fn message_terms(value: &str) -> HashSet<String> {
    let normalized = value.to_lowercase();
    let mut terms = HashSet::new();
    for segment in normalized.split(|character: char| !character.is_alphanumeric()) {
        if segment.is_ascii() {
            if segment.len() >= 2 {
                terms.insert(segment.to_string());
            }
            continue;
        }
        let characters = segment.chars().collect::<Vec<_>>();
        for pair in characters.windows(2) {
            terms.insert(pair.iter().collect());
        }
    }
    terms
}

fn wire_message_rejection_reason(message: &str) -> Option<&'static str> {
    let message = message.trim();
    let chars = message.chars().count();
    if chars < 12 {
        return Some("成品不足 12 字，未发布快讯");
    }
    if chars > 220 {
        return Some("成品超过 220 字，未发布快讯");
    }
    if !message
        .chars()
        .any(|character| ('\u{4e00}'..='\u{9fff}').contains(&character))
    {
        return Some("成品不含中文，未发布快讯");
    }
    if [
        "他们",
        "她们",
        "它们",
        "他",
        "她",
        "它",
        "其",
        "此案",
        "该案",
        "该事件",
        "警方",
        "当局",
        "有关方面",
        "消息人士",
    ]
    .iter()
    .any(|prefix| message.starts_with(prefix))
    {
        return Some("成品缺少独立可读的明确主体，未发布快讯");
    }
    None
}

fn source_family_from_url(url: &str) -> Option<String> {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
        .map(|host| host.strip_prefix("www.").unwrap_or(&host).to_string())
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clip_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

fn retry_delay_seconds(run_id: Uuid, attempts: i32) -> i64 {
    let exponent = attempts.saturating_sub(1).clamp(0, 6) as u32;
    let base = (30_i64.saturating_mul(2_i64.pow(exponent))).min(1_800);
    let jitter = 800 + (run_id.as_u128() % 201) as i64;
    base.saturating_mul(jitter) / 1_000
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::GeneratedCitation;

    fn summary(corroboration: &str, grounding_score: Option<i32>) -> GeneratedSummary {
        GeneratedSummary {
            summary: "两家媒体报道同一事件。".to_string(),
            bullets: Vec::new(),
            keywords: Vec::new(),
            confidence: None,
            confidence_reason: None,
            citations: vec![
                GeneratedCitation {
                    claim_id: "summary".to_string(),
                    evidence_id: "S1:E001".to_string(),
                    article_id: Uuid::nil(),
                    source_title: "A".to_string(),
                    article_title: "A".to_string(),
                    url: "https://a.example".to_string(),
                    excerpt: "证据 A".to_string(),
                    verified: true,
                },
                GeneratedCitation {
                    claim_id: "summary".to_string(),
                    evidence_id: "S2:E001".to_string(),
                    article_id: Uuid::from_u128(1),
                    source_title: "B".to_string(),
                    article_title: "B".to_string(),
                    url: "https://b.example".to_string(),
                    excerpt: "证据 B".to_string(),
                    verified: true,
                },
            ],
            disagreements: Vec::new(),
            grounding_score,
            corroboration: corroboration.to_string(),
            method: "grounded-event-v3".to_string(),
            scope: "event_snapshot".to_string(),
            schema_version: 2,
            source_count: 3,
            cited_source_count: 2,
        }
    }

    #[test]
    fn labels_multi_source_grounded_event_as_corroborated() {
        let generated = summary("multi", Some(88));
        let verification = classify_verification(
            &generated,
            "CONFIRM",
            core_cited_source_count(&generated, &[]),
        );
        assert_eq!(verification.status, "corroborated");
        assert_eq!(verification.label, "多源一致");
        assert_eq!(verification.score, 88);
    }

    #[test]
    fn direct_multi_source_support_is_not_downgraded_by_discarded_draft_claims() {
        let generated = summary("multi", Some(63));
        let verification = classify_verification(
            &generated,
            "CONFIRM",
            core_cited_source_count(&generated, &[]),
        );
        assert_eq!(verification.status, "corroborated");
        assert_eq!(verification.score, 80);
    }

    #[test]
    fn disagreement_never_becomes_corroborated() {
        let mut generated = summary("divergent", Some(92));
        generated.disagreements.push("数字不一致".to_string());
        let verification = classify_verification(
            &generated,
            "CONTRADICT",
            core_cited_source_count(&generated, &[]),
        );
        assert_eq!(verification.status, "disputed");
        assert!(verification.score <= 70);
    }

    #[test]
    fn unrelated_citations_do_not_make_the_core_claim_corroborated() {
        let mut generated = summary("multi", Some(90));
        generated.citations[1].claim_id = "bullet:0".to_string();
        let core_sources = core_cited_source_count(&generated, &[]);
        let verification = classify_verification(&generated, "CONFIRM", core_sources);
        assert_eq!(core_sources, 1);
        assert_eq!(verification.status, "developing");
    }

    #[test]
    fn duplicate_articles_from_one_publisher_count_as_one_core_source() {
        let mut generated = summary("multi", Some(90));
        generated.citations[1].url = "https://www.a.example/another-path".to_string();
        assert_eq!(core_cited_source_count(&generated, &[]), 1);
    }

    #[test]
    fn compact_message_is_bounded() {
        let message = compact_message(&"甲".repeat(250), &[], &[], "developing");
        assert_eq!(message.chars().count(), 220);
    }

    #[test]
    fn compact_message_includes_one_verified_supporting_point() {
        let message = compact_message(
            "救援队将失去行动能力的东京送至兽医处。",
            &[
                "兽医认为这只拉布拉多犬误食了遗留在山道上的大麻。".to_string(),
                "东京次日完全康复。".to_string(),
            ],
            &[],
            "developing",
        );
        assert!(message.contains("误食了遗留在山道上的大麻"));
        assert!(!message.contains("次日完全康复"));
    }

    #[test]
    fn compact_message_drops_an_already_included_point_with_terminal_punctuation() {
        let message = compact_message(
            "中东危机：特朗普称霍尔木兹海峡对商业交通开放，尽管伊朗早些时候宣布关闭。",
            &["特朗普称霍尔木兹海峡对商业交通开放，尽管伊朗早些时候宣布关闭。".to_string()],
            &[],
            "developing",
        );
        assert_eq!(
            message,
            "中东危机：特朗普称霍尔木兹海峡对商业交通开放，尽管伊朗早些时候宣布关闭"
        );
    }

    #[test]
    fn compact_message_drops_a_paraphrase_of_the_core_claim() {
        let message = compact_message(
            "特朗普因加拿大山火烟雾进入美国城市，威胁征收新关税。",
            &["特朗普威胁对加拿大征收新关税，作为野火烟雾飘入美国的惩罚。".to_string()],
            &[],
            "corroborated",
        );
        assert_eq!(
            message,
            "特朗普因加拿大山火烟雾进入美国城市，威胁征收新关税"
        );
    }

    #[test]
    fn wire_focus_title_removes_live_blog_wrappers_only() {
        assert_eq!(
            wire_focus_title("Middle East crisis live: Strait remains open – as it happened"),
            "Middle East crisis live: Strait remains open"
        );
        assert_eq!(
            wire_focus_title("Live updates: Strait remains open"),
            "Strait remains open"
        );
        assert_eq!(
            wire_focus_title("Infantino opens door to 64-team World Cup"),
            "Infantino opens door to 64-team World Cup"
        );
        assert_eq!(
            wire_focus_title("Saturday Night Live: cold open recap"),
            "Saturday Night Live: cold open recap"
        );
        assert_eq!(
            wire_focus_title("Xbox Live: service status restored"),
            "Xbox Live: service status restored"
        );
        assert_eq!(
            wire_focus_title("CueBench for Developers is live: score coding agents"),
            "CueBench for Developers is live: score coding agents"
        );
        assert_eq!(
            wire_focus_title("Product goes officially live: launch details"),
            "Product goes officially live: launch details"
        );
    }

    #[test]
    fn disputed_message_never_truncates_a_disagreement() {
        let message = compact_message(
            &"甲".repeat(200),
            &["这条要点不应挤占分歧空间。".to_string()],
            &["乙".repeat(80)],
            "disputed",
        );
        assert_eq!(message, "甲".repeat(200));
        assert!(!message.contains("来源说法不一"));
    }

    #[test]
    fn disputed_message_exposes_the_disagreement() {
        let message = compact_message(
            "伊朗袭击了美国在海湾地区的军事设施。",
            &[],
            &["目标范围：来源甲称巴林、科威特；来源乙还包括卡塔尔。".to_string()],
            "disputed",
        );
        assert!(message.contains("来源说法不一"));
        assert!(message.contains("卡塔尔"));
    }

    #[test]
    fn standalone_wire_message_rejects_ambiguous_subjects_and_non_chinese_text() {
        assert_eq!(
            wire_message_rejection_reason("他以鹰派外交政策立场著称，支持对伊朗采取军事行动"),
            Some("成品缺少独立可读的明确主体，未发布快讯")
        );
        assert_eq!(
            wire_message_rejection_reason("警方尚未逮捕任何人，调查仍在进行中"),
            Some("成品缺少独立可读的明确主体，未发布快讯")
        );
        assert_eq!(
            wire_message_rejection_reason("Breaking news with enough characters"),
            Some("成品不含中文，未发布快讯")
        );
        assert_eq!(
            wire_message_rejection_reason("台风已减弱为强热带风暴，但仍具危险性"),
            None
        );
    }
}
