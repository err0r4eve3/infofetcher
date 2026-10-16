//! Durable jobs with frozen inputs, expiring leases and fenced completion.
use super::*;
use axum::http::StatusCode;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v2/events/{id}/osint/jobs", post(queue_event))
        .route("/api/v2/osint/topics", post(queue_topic))
        .route("/api/v2/osint/cases/{id}/jobs", get(list).post(queue_case))
        .route("/api/v2/osint/jobs/{id}/cancel", post(cancel))
        .route("/api/v2/osint/jobs/{id}/retry", post(retry))
        .layer(axum::middleware::map_response(
            |mut r: Response| async move {
                r.headers_mut()
                    .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
                r
            },
        ))
}
async fn queue_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<research::ResearchRequest>,
) -> AppResult<impl IntoResponse> {
    let actor = auth::require_session(&state.pool, &headers).await?;
    let mut tx = state.pool.begin().await?;
    let title: String =
        sqlx::query_scalar("SELECT representative_title FROM story_clusters WHERE id=$1")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(AppError::NotFound)?;
    let case_id = casebook::ensure_case(&mut tx, id, &title).await?;
    // Publish the case before preparing input via the shared pool.
    tx.commit().await?;
    queue(&state, case_id, &actor, request).await
}
// Active search for an event that has no cluster yet: the query names the case.
async fn queue_topic(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<research::ResearchRequest>,
) -> AppResult<impl IntoResponse> {
    let actor = auth::require_session(&state.pool, &headers).await?;
    let title = research::normalized_query(request.query.as_deref().unwrap_or_default())?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("osint-topic:{}", title.to_lowercase()))
        .execute(&mut *tx)
        .await?;
    let existing: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM osint_cases WHERE origin = 'topic' AND lower(title) = lower($1)
        ORDER BY updated_at DESC LIMIT 1",
    )
    .bind(&title)
    .fetch_optional(&mut *tx)
    .await?;
    let case_id: Uuid = match existing {
        Some(id) => {
            sqlx::query("UPDATE osint_cases SET updated_at = now() WHERE id = $1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            id
        }
        None => {
            sqlx::query_scalar(
                "INSERT INTO osint_cases (event_id, title, origin)
                VALUES (gen_random_uuid(), $1, 'topic') RETURNING id",
            )
            .bind(&title)
            .fetch_one(&mut *tx)
            .await?
        }
    };
    tx.commit().await?;
    queue(&state, case_id, &actor, request).await
}
async fn queue_case(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<research::ResearchRequest>,
) -> AppResult<impl IntoResponse> {
    let actor = auth::require_session(&state.pool, &headers).await?;
    queue(&state, id, &actor, request).await
}
async fn queue(
    state: &AppState,
    case_id: Uuid,
    actor: &str,
    request: research::ResearchRequest,
) -> AppResult<(StatusCode, Json<Value>)> {
    let mut tx = state.pool.begin().await?;
    let event_id: Uuid =
        sqlx::query_scalar("SELECT event_id FROM osint_cases WHERE id=$1 FOR UPDATE")
            .bind(case_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(AppError::NotFound)?;
    if let Some(id)=sqlx::query_scalar::<_,Uuid>("SELECT id FROM osint_jobs WHERE case_id=$1 AND kind='research' AND status IN ('queued','running')").bind(case_id).fetch_optional(&mut *tx).await? {
        return Ok((StatusCode::ACCEPTED,Json(json!({"id":id,"case_id":case_id,"existing":true}))));
    }
    let input = tokio::time::timeout(
        Duration::from_secs(20),
        research::prepare(state, event_id, request, Some(case_id)),
    )
    .await
    .map_err(|_| AppError::Upstream("冻结调查输入超时".into()))??;
    let input = serde_json::to_value(input).map_err(|e| AppError::Anyhow(e.into()))?;
    let id:Uuid=sqlx::query_scalar("INSERT INTO osint_jobs (case_id,kind,input,created_by) VALUES ($1,'research',$2,$3) RETURNING id")
        .bind(case_id).bind(input).bind(actor).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"id":id,"case_id":case_id,"existing":false})),
    ))
}
async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    auth::require_session(&state.pool, &headers).await?;
    let items:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(j)-'input'-'lease_token' FROM osint_jobs j WHERE case_id=$1 ORDER BY (status IN ('queued','running')) DESC,created_at DESC,id DESC LIMIT 50")
        .bind(id).fetch_all(&state.pool).await?;
    Ok(Json(json!({"items":items})))
}
async fn cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    auth::require_session(&state.pool, &headers).await?;
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query("SELECT status,document_id FROM osint_jobs WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;
    if ["queued", "running"].contains(&row.get::<String, _>("status").as_str()) {
        sqlx::query("UPDATE osint_jobs SET status='cancelled',lease_token=NULL,lease_until=NULL,updated_at=now() WHERE id=$1").bind(id).execute(&mut *tx).await?;
        if let Some(doc) = row.get::<Option<Uuid>, _>("document_id") {
            sqlx::query("UPDATE osint_documents SET status='cancelled' WHERE id=$1")
                .bind(doc)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(Json(json!({"ok":true})))
}
async fn retry(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    auth::require_session(&state.pool, &headers).await?;
    let mut tx = state.pool.begin().await?;
    let case_id: Uuid = sqlx::query_scalar("SELECT case_id FROM osint_jobs WHERE id=$1")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;
    sqlx::query("SELECT id FROM osint_cases WHERE id=$1 FOR UPDATE")
        .bind(case_id)
        .execute(&mut *tx)
        .await?;
    let row = sqlx::query("SELECT kind,status,document_id FROM osint_jobs WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    if !["failed", "cancelled"].contains(&row.get::<String, _>("status").as_str()) {
        return Err(AppError::BadRequest("仅失败或取消的任务可以重试".into()));
    }
    if row.get::<String, _>("kind") == "research" {
        let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM osint_jobs WHERE case_id=$1 AND kind='research' AND status IN ('queued','running'))").bind(case_id).fetch_one(&mut *tx).await?;
        if active {
            return Err(AppError::RateLimited("档案已有进行中的调查".into()));
        }
    }
    sqlx::query("UPDATE osint_jobs SET status='queued',attempts=0,error=NULL,lease_token=NULL,lease_until=NULL,updated_at=now() WHERE id=$1").bind(id).execute(&mut *tx).await?;
    if let Some(doc) = row.get::<Option<Uuid>, _>("document_id") {
        sqlx::query("UPDATE osint_documents SET status='queued' WHERE id=$1")
            .bind(doc)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(Json(json!({"ok":true})))
}

pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        loop {
            match run_once(&state).await {
                Ok(true) => continue,
                Ok(false) => {}
                Err(_) => tracing::warn!("OSINT queue database operation failed; retrying"),
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
}

pub(super) async fn run_once(state: &AppState) -> AppResult<bool> {
    // Three interrupted attempts exhaust automatic crash recovery. Explicit retry
    // is available after inspecting the error. No source/model calls occur here.
    sqlx::query("WITH failed AS (UPDATE osint_jobs SET status='failed',error='任务连续中断 3 次，请检查服务后重试',lease_token=NULL,lease_until=NULL,updated_at=now() WHERE status='running' AND lease_until<now() AND attempts>=3 RETURNING document_id) UPDATE osint_documents SET status='failed' WHERE id IN (SELECT document_id FROM failed)")
        .execute(&state.pool).await?;
    let token = Uuid::new_v4();
    let row=sqlx::query("UPDATE osint_jobs j SET status='running',attempts=attempts+1,lease_token=$1,lease_until=now()+interval '10 minutes',updated_at=now(),error=NULL FROM (SELECT id FROM osint_jobs WHERE status='queued' OR (status='running' AND lease_until<now() AND attempts<3) ORDER BY created_at,id FOR UPDATE SKIP LOCKED LIMIT 1) picked WHERE j.id=picked.id RETURNING j.*")
        .bind(token).fetch_optional(&state.pool).await?;
    let Some(row) = row else {
        return Ok(false);
    };
    let id: Uuid = row.get("id");
    let case_id: Uuid = row.get("case_id");
    let doc: Option<Uuid> = row.get("document_id");
    if let Some(doc) = doc {
        sqlx::query("UPDATE osint_documents SET status='running' WHERE id=$1 AND EXISTS(SELECT 1 FROM osint_jobs WHERE id=$2 AND lease_token=$3 AND status='running')")
            .bind(doc).bind(id).bind(token).execute(&state.pool).await?;
    }
    let work = async {
        if let Some(doc) = doc {
            let source = sqlx::query("SELECT media_type,body FROM osint_documents WHERE id=$1")
                .bind(doc)
                .fetch_one(&state.pool)
                .await?;
            documents::extract(
                &source.get::<String, _>("media_type"),
                &source.get::<Vec<u8>, _>("body"),
            )
            .await
            .map_err(AppError::Anyhow)
        } else {
            research::execute(state, row.get("input"), id, case_id).await
        }
    };
    let result = tokio::select! {
        result=tokio::time::timeout(Duration::from_secs(300),work)=>result.unwrap_or_else(|_|Err(AppError::Upstream("任务执行超过 5 分钟，已停止；原始输入保留".into()))),
        _=lost_lease(state,id,token)=>return Ok(true),
    };
    let mut tx = state.pool.begin().await?;
    // Keep the same case -> job lock order used by explicit retry.
    sqlx::query("SELECT id FROM osint_cases WHERE id=$1 FOR UPDATE")
        .bind(case_id)
        .execute(&mut *tx)
        .await?;
    // Lock + token fencing prevents a cancelled/expired worker from committing.
    let owned:Option<Uuid>=sqlx::query_scalar("SELECT id FROM osint_jobs WHERE id=$1 AND status='running' AND lease_token=$2 AND lease_until>clock_timestamp() FOR UPDATE")
        .bind(id).bind(token).fetch_optional(&mut *tx).await?;
    if owned.is_none() {
        return Ok(true);
    }
    match result {
        Ok(value) => {
            let hash = hex::encode(Sha256::digest(value.to_string().as_bytes()));
            if let Some(doc) = doc {
                sqlx::query("UPDATE osint_documents SET status='ready',extraction=$2,extraction_sha256=$3 WHERE id=$1")
                    .bind(doc).bind(value).bind(hash).execute(&mut *tx).await?;
            } else {
                sqlx::query("INSERT INTO osint_runs (id,case_id,created_by,report,report_sha256) VALUES ($1,$2,$3,$4,$5)")
                    .bind(id).bind(case_id).bind(row.get::<String,_>("created_by")).bind(value).bind(hash).execute(&mut *tx).await?;
            }
            sqlx::query("UPDATE osint_jobs SET status='done',run_id=$2,lease_token=NULL,lease_until=NULL,updated_at=now() WHERE id=$1")
                .bind(id).bind(if doc.is_none(){Some(id)}else{None}).execute(&mut *tx).await?;
            sqlx::query("UPDATE osint_cases SET updated_at=now() WHERE id=$1")
                .bind(case_id)
                .execute(&mut *tx)
                .await?;
        }
        Err(error) => {
            let message = match error {
                AppError::Anyhow(e) => e.to_string(),
                AppError::Upstream(e) => e,
                _ => "本地处理失败，请检查服务日志与数据库状态".into(),
            };
            sqlx::query("UPDATE osint_jobs SET status='failed',error=$2,lease_token=NULL,lease_until=NULL,updated_at=now() WHERE id=$1")
                .bind(id).bind(message.chars().take(500).collect::<String>()).execute(&mut *tx).await?;
            if let Some(doc) = doc {
                sqlx::query("UPDATE osint_documents SET status='failed' WHERE id=$1")
                    .bind(doc)
                    .execute(&mut *tx)
                    .await?;
            }
        }
    }
    tx.commit().await?;
    Ok(true)
}
async fn lost_lease(state: &AppState, id: Uuid, token: Uuid) {
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let owned=sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM osint_jobs WHERE id=$1 AND status='running' AND lease_token=$2 AND lease_until>clock_timestamp())")
            .bind(id).bind(token).fetch_one(&state.pool).await;
        if !matches!(owned, Ok(true)) {
            return;
        }
    }
}
