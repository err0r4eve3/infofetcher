//! Durable local investigations. Source IDs refer to immutable snapshots, not
//! reader rows: deleting a feed or applying reader retention cannot erase a case.
use super::*;
use axum::extract::Query;
use sqlx::PgConnection;

static SCAN_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v2/osint/cases", get(list_cases))
        .route("/api/v2/osint/cases/{id}", get(case_detail))
        .route("/api/v2/events/{id}/osint/history", get(event_history))
        .route("/api/v2/osint/runs/{id}", get(run_detail))
        .route("/api/v2/osint/runs/{id}/export", get(export_run))
        .route("/api/v2/osint/runs/{id}/reviews", post(review))
        .route("/api/v2/osint/runs/{id}/entities", post(link_entity))
        .route("/api/v2/osint/runs/{id}/relations", post(add_relation))
        .route("/api/v2/osint/entities", get(list_entities))
        .route("/api/v2/osint/entities/{id}", get(entity_detail))
        .route("/api/v2/osint/entities/{id}/scan", post(scan_entity))
        .route("/api/v2/osint/mentions/{id}/review", post(review_mention))
        .route("/api/v2/osint/relations/{id}/review", post(review_relation))
        .layer(axum::middleware::map_response(
            |mut response: Response| async move {
                response
                    .headers_mut()
                    .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
                response
            },
        ))
}

pub(super) async fn ensure_case(
    conn: &mut PgConnection,
    event_id: Uuid,
    title: &str,
) -> AppResult<Uuid> {
    Ok(sqlx::query_scalar(
        "INSERT INTO osint_cases (event_id, title) VALUES ($1, $2)
        ON CONFLICT (event_id) DO UPDATE SET updated_at = now() RETURNING id",
    )
    .bind(event_id)
    .bind(title)
    .fetch_one(conn)
    .await?)
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    #[serde(default)]
    offset: i64,
    #[serde(default)]
    q: String,
}

impl Page {
    fn validate(&self) -> AppResult<()> {
        if !(0..=1_000_000).contains(&self.offset) || self.q.chars().count() > 240 {
            return Err(AppError::BadRequest("分页或检索词超出范围".into()));
        }
        Ok(())
    }
}

async fn list_cases(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(page): Query<Page>,
) -> AppResult<Json<Value>> {
    auth::require_session(&state.pool, &headers).await?;
    page.validate()?;
    let mut items: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(c) || jsonb_build_object(
        'event_available', EXISTS(SELECT 1 FROM story_clusters s WHERE s.id = c.event_id),
        'run_count', (SELECT count(*) FROM osint_runs r WHERE r.case_id = c.id),
        'capture_count', (SELECT count(*) FROM osint_captures x WHERE x.case_id = c.id),
        'document_count', (SELECT count(*) FROM osint_documents d WHERE d.case_id = c.id))
        FROM osint_cases c WHERE $1 = '' OR strpos(lower(c.title), lower($1)) > 0
        ORDER BY c.updated_at DESC, c.id DESC LIMIT 51 OFFSET $2",
    )
    .bind(page.q.trim())
    .bind(page.offset)
    .fetch_all(&state.pool)
    .await?;
    let more = items.len() > 50;
    items.truncate(50);
    Ok(Json(
        json!({"items": items, "next_offset": more.then_some(page.offset + 50)}),
    ))
}

async fn case_data(state: &AppState, id: Uuid, offset: i64) -> AppResult<Value> {
    let case: Value = sqlx::query_scalar(
        "SELECT to_jsonb(c) || jsonb_build_object(
        'event_available', EXISTS(SELECT 1 FROM story_clusters s WHERE s.id = c.event_id))
        FROM osint_cases c WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    let mut runs: Vec<Value> = sqlx::query_scalar(
        "SELECT jsonb_build_object('id', id,
        'created_at', created_at, 'created_by', created_by, 'query', report->>'query',
        'status', report->>'status', 'material_count', jsonb_array_length(report->'materials'),
        'report_sha256', report_sha256) FROM osint_runs WHERE case_id = $1
        ORDER BY created_at DESC, id DESC LIMIT 51 OFFSET $2",
    )
    .bind(id)
    .bind(offset)
    .fetch_all(&state.pool)
    .await?;
    let mut captures: Vec<Value> = sqlx::query_scalar(
        "SELECT jsonb_build_object('id', id, 'article_id', article_id,
        'captured_at', captured_at, 'sha256', sha256, 'details', details,
        'byte_length', octet_length(body)) FROM osint_captures c WHERE case_id = $1
        ORDER BY captured_at DESC, id DESC LIMIT 51 OFFSET $2",
    )
    .bind(id)
    .bind(offset)
    .fetch_all(&state.pool)
    .await?;
    let more = runs.len() > 50 || captures.len() > 50;
    runs.truncate(50);
    captures.truncate(50);
    Ok(
        json!({"case": case, "runs": runs, "captures": captures, "next_offset": more.then_some(offset + 50)}),
    )
}

async fn case_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(page): Query<Page>,
) -> AppResult<Json<Value>> {
    auth::require_session(&state.pool, &headers).await?;
    page.validate()?;
    Ok(Json(case_data(&state, id, page.offset).await?))
}

async fn event_history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    auth::require_session(&state.pool, &headers).await?;
    let case_id: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM osint_cases WHERE event_id = $1")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
    Ok(Json(match case_id {
        Some(id) => case_data(&state, id, 0).await?,
        None => Value::Null,
    }))
}

async fn report(state: &AppState, id: Uuid) -> AppResult<Value> {
    sqlx::query_scalar("SELECT report FROM osint_runs WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)
}

async fn run_bundle(state: &AppState, id: Uuid) -> AppResult<Value> {
    let mut run: Value = sqlx::query_scalar("SELECT to_jsonb(r) FROM osint_runs r WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;
    let reviews: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(r) FROM osint_reviews r WHERE run_id = $1 ORDER BY id DESC",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;
    let mentions: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(m) || jsonb_build_object('entity', to_jsonb(e))
        FROM osint_mentions m JOIN osint_entities e ON e.id = m.entity_id WHERE run_id = $1 ORDER BY m.created_at, m.id")
        .bind(id).fetch_all(&state.pool).await?;
    let relations: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(r) || jsonb_build_object('subject_name', s.name, 'object_name', o.name)
        FROM osint_relations r JOIN osint_entities s ON s.id = r.subject_id JOIN osint_entities o ON o.id = r.object_id
        WHERE run_id = $1 ORDER BY r.created_at, r.id")
        .bind(id).fetch_all(&state.pool).await?;
    let audit: Vec<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(a) FROM osint_entity_audit a WHERE run_id = $1 ORDER BY id DESC",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;
    run["reviews"] = json!(reviews);
    run["mentions"] = json!(mentions);
    run["relations"] = json!(relations);
    run["entity_audit"] = json!(audit);
    Ok(run)
}

async fn run_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    auth::require_session(&state.pool, &headers).await?;
    Ok(Json(run_bundle(&state, id).await?))
}

async fn export_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> AppResult<Response> {
    auth::require_session(&state.pool, &headers).await?;
    Ok((
        [
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"investigation-{id}.json\""),
            ),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".into()),
        ],
        Json(json!({"format": "infofetcher-case-export-v1", "run": run_bundle(&state, id).await?})),
    )
        .into_response())
}

fn bounded_text(raw: &str, label: &str, min: usize, max: usize) -> AppResult<String> {
    let value = raw.trim();
    if !(min..=max).contains(&value.chars().count())
        || value.chars().any(|c| c.is_control() && !c.is_whitespace())
    {
        return Err(AppError::BadRequest(format!(
            "{label}须为 {min}–{max} 字符"
        )));
    }
    Ok(value.to_string())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewRequest {
    target_kind: String,
    target_index: usize,
    verdict: String,
    note: String,
}

async fn review(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<ReviewRequest>,
) -> AppResult<Json<Value>> {
    let actor = auth::require_session(&state.pool, &headers).await?;
    let data = report(&state, id).await?;
    let key = match request.target_kind.as_str() {
        "finding" => "findings",
        "fragment" => "fragments",
        _ => return Err(AppError::BadRequest("复核对象无效".into())),
    };
    if data[key]
        .as_array()
        .and_then(|a| a.get(request.target_index))
        .is_none()
        || !["confirmed", "rejected", "needs_review"].contains(&request.verdict.as_str())
    {
        return Err(AppError::BadRequest("复核对象或结论无效".into()));
    }
    let note = bounded_text(&request.note, "复核理由", 1, 2000)?;
    let row: Value = sqlx::query_scalar(
        "INSERT INTO osint_reviews (run_id, target_kind, target_index, verdict, note, actor)
        VALUES ($1, $2, $3, $4, $5, $6) RETURNING to_jsonb(osint_reviews.*)",
    )
    .bind(id)
    .bind(request.target_kind)
    .bind(request.target_index as i32)
    .bind(request.verdict)
    .bind(note)
    .bind(actor)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(row))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EntityRequest {
    entity_id: Option<Uuid>,
    kind: Option<String>,
    name: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    article_id: Uuid,
    excerpt: String,
}

fn aliases(name: &str, values: &[String]) -> AppResult<Vec<String>> {
    if values.len() > 12 {
        return Err(AppError::BadRequest("最多 12 个别名".into()));
    }
    let mut result = vec![bounded_text(name, "实体名称", 2, 120)?];
    for value in values {
        let alias = bounded_text(value, "别名", 2, 120)?;
        if !result
            .iter()
            .any(|s| s.to_lowercase() == alias.to_lowercase())
        {
            result.push(alias);
        }
    }
    Ok(result)
}

fn evidence_excerpt(data: &Value, article_id: Uuid, raw: &str) -> AppResult<String> {
    let excerpt = bounded_text(raw, "原文片段", 2, 1000)?;
    let article = article_id.to_string();
    let valid = data["materials"].as_array().is_some_and(|items| {
        items.iter().any(|m| {
            m["id"].as_str() == Some(&article)
                && m["content"].as_str().is_some_and(|s| s.contains(&excerpt))
        })
    });
    if !valid {
        return Err(AppError::BadRequest(
            "片段必须逐字出现在本次调查保存的材料中".into(),
        ));
    }
    Ok(excerpt)
}

// Match Latin aliases at word boundaries; CJK aliases may occur in continuous
// text. A textual match is only a candidate, never automatic identity resolution.
fn alias_excerpt(text: &str, alias: &str) -> Option<String> {
    let needle: Vec<char> = alias.to_lowercase().chars().collect();
    let original: Vec<char> = text.chars().collect();
    if needle.is_empty() {
        return None;
    }
    // Keep an index into the original text even when Unicode case folding expands.
    let mut lower = Vec::new();
    let mut positions = Vec::new();
    for (i, c) in original.iter().enumerate() {
        for folded in c.to_lowercase() {
            lower.push(folded);
            positions.push(i);
        }
    }
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    for (i, window) in lower.windows(needle.len()).enumerate() {
        if window != needle {
            continue;
        }
        let end = i + needle.len();
        if (word(needle[0]) && i > 0 && word(lower[i - 1]))
            || (word(*needle.last().unwrap()) && end < lower.len() && word(lower[end]))
        {
            continue;
        }
        let start = positions[i].saturating_sub(70);
        let stop = (positions[end - 1] + 71).min(original.len());
        return Some(original[start..stop].iter().collect());
    }
    None
}

async fn audit(
    conn: &mut PgConnection,
    run_id: Uuid,
    actor: &str,
    action: &str,
    details: Value,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO osint_entity_audit (run_id, actor, action, details) VALUES ($1, $2, $3, $4)",
    )
    .bind(run_id)
    .bind(actor)
    .bind(action)
    .bind(details)
    .execute(conn)
    .await?;
    Ok(())
}

async fn link_entity(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<EntityRequest>,
) -> AppResult<Json<Value>> {
    let actor = auth::require_session(&state.pool, &headers).await?;
    let data = report(&state, id).await?;
    let excerpt = evidence_excerpt(&data, request.article_id, &request.excerpt)?;
    let mut tx = state.pool.begin().await?;
    let (entity_id, names) = if let Some(entity_id) = request.entity_id {
        if request.name.is_some() || request.kind.is_some() || !request.aliases.is_empty() {
            return Err(AppError::BadRequest("关联已有实体时不能改写其身份".into()));
        }
        let names: Value = sqlx::query_scalar("SELECT aliases FROM osint_entities WHERE id = $1")
            .bind(entity_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(AppError::NotFound)?;
        (
            entity_id,
            serde_json::from_value::<Vec<String>>(names).map_err(|e| AppError::Anyhow(e.into()))?,
        )
    } else {
        let name = bounded_text(request.name.as_deref().unwrap_or(""), "实体名称", 2, 120)?;
        let kind = request.kind.as_deref().unwrap_or("");
        if !["person", "organization", "location"].contains(&kind) {
            return Err(AppError::BadRequest("实体类型无效".into()));
        }
        let names = aliases(&name, &request.aliases)?;
        let entity_id: Uuid = sqlx::query_scalar(
            "INSERT INTO osint_entities (kind, name, aliases, created_by)
            VALUES ($1, $2, $3, $4) RETURNING id",
        )
        .bind(kind)
        .bind(name)
        .bind(json!(names))
        .bind(&actor)
        .fetch_one(&mut *tx)
        .await?;
        (entity_id, names)
    };
    let matched = names
        .iter()
        .find(|a| alias_excerpt(&excerpt, a).is_some())
        .ok_or_else(|| AppError::BadRequest("片段中未找到实体名称或别名".into()))?;
    let mention_id: Uuid = sqlx::query_scalar("INSERT INTO osint_mentions (entity_id, run_id, article_id, matched_alias, excerpt, status)
        VALUES ($1, $2, $3, $4, $5, 'confirmed') ON CONFLICT (entity_id, run_id, article_id, matched_alias)
        DO UPDATE SET status = 'confirmed', excerpt = EXCLUDED.excerpt RETURNING id")
        .bind(entity_id).bind(id).bind(request.article_id).bind(matched).bind(&excerpt).fetch_one(&mut *tx).await?;
    audit(
        &mut tx,
        id,
        &actor,
        "entity_linked",
        json!({"entity_id": entity_id, "mention_id": mention_id,
        "article_id": request.article_id, "excerpt": excerpt, "aliases": names}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"entity_id": entity_id, "mention_id": mention_id}),
    ))
}

async fn list_entities(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(page): Query<Page>,
) -> AppResult<Json<Value>> {
    auth::require_session(&state.pool, &headers).await?;
    page.validate()?;
    let mut items: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(e) FROM osint_entities e WHERE $1 = ''
        OR EXISTS (SELECT 1 FROM jsonb_array_elements_text(aliases) a WHERE strpos(lower(a), lower($1)) > 0)
        ORDER BY created_at DESC, id DESC LIMIT 51 OFFSET $2")
        .bind(page.q.trim()).bind(page.offset).fetch_all(&state.pool).await?;
    let more = items.len() > 50;
    items.truncate(50);
    Ok(Json(
        json!({"items": items, "next_offset": more.then_some(page.offset + 50)}),
    ))
}

async fn entity_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(page): Query<Page>,
) -> AppResult<Json<Value>> {
    auth::require_session(&state.pool, &headers).await?;
    page.validate()?;
    let entity: Value =
        sqlx::query_scalar("SELECT to_jsonb(e) FROM osint_entities e WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or(AppError::NotFound)?;
    let mut items: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(m) || jsonb_build_object('case_id', c.id, 'case_title', c.title)
        FROM osint_mentions m JOIN osint_runs r ON r.id = m.run_id JOIN osint_cases c ON c.id = r.case_id
        WHERE entity_id = $1 ORDER BY m.created_at DESC, m.id DESC LIMIT 51 OFFSET $2")
        .bind(id).bind(page.offset).fetch_all(&state.pool).await?;
    let more = items.len() > 50;
    items.truncate(50);
    Ok(Json(
        json!({"entity": entity, "mentions": items, "next_offset": more.then_some(page.offset + 50)}),
    ))
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScanRequest {
    before: Option<Uuid>,
}

async fn scan_entity(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<ScanRequest>,
) -> AppResult<Json<Value>> {
    let actor = auth::require_session(&state.pool, &headers).await?;
    let _slot = SCAN_SLOTS
        .try_acquire()
        .map_err(|_| AppError::RateLimited("已有实体检索正在运行".into()))?;
    let names: Value = sqlx::query_scalar("SELECT aliases FROM osint_entities WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;
    let names: Vec<String> =
        serde_json::from_value(names).map_err(|e| AppError::Anyhow(e.into()))?;
    if let Some(before) = request.before {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM osint_runs WHERE id = $1)")
                .bind(before)
                .fetch_one(&state.pool)
                .await?;
        if !exists {
            return Err(AppError::NotFound);
        }
    }
    let rows = sqlx::query("SELECT id, report FROM osint_runs WHERE $1::uuid IS NULL OR (created_at, id) <
        (SELECT created_at, id FROM osint_runs WHERE id = $1) ORDER BY created_at DESC, id DESC LIMIT 21")
        .bind(request.before).fetch_all(&state.pool).await?;
    let more = rows.len() > 20;
    let runs: Vec<(Uuid, Value)> = rows
        .into_iter()
        .take(20)
        .map(|r| (r.get("id"), r.get("report")))
        .collect();
    let scanned = runs.len();
    let next = if more { runs.last().map(|r| r.0) } else { None };
    let matches = tokio::task::spawn_blocking(move || {
        let mut matches = vec![];
        for (run_id, data) in runs {
            if let Some(materials) = data["materials"].as_array() {
                for m in materials {
                    let (Some(article), Some(text)) = (
                        m["id"].as_str().and_then(|s| Uuid::parse_str(s).ok()),
                        m["content"].as_str(),
                    ) else {
                        continue;
                    };
                    for alias in &names {
                        if let Some(excerpt) = alias_excerpt(text, alias) {
                            matches.push((run_id, article, alias.clone(), excerpt));
                            break;
                        }
                    }
                }
            }
        }
        matches
    })
    .await
    .map_err(|e| AppError::Anyhow(e.into()))?;
    let mut tx = state.pool.begin().await?;
    let mut added = 0;
    for (run, article, alias, excerpt) in matches {
        let inserted = sqlx::query("INSERT INTO osint_mentions (entity_id, run_id, article_id, matched_alias, excerpt, status)
            VALUES ($1, $2, $3, $4, $5, 'suggested') ON CONFLICT DO NOTHING")
            .bind(id).bind(run).bind(article).bind(alias).bind(excerpt).execute(&mut *tx).await?.rows_affected();
        if inserted > 0 {
            audit(
                &mut tx,
                run,
                &actor,
                "alias_candidate",
                json!({"entity_id": id, "article_id": article}),
            )
            .await?;
            added += inserted;
        }
    }
    tx.commit().await?;
    Ok(Json(
        json!({"scanned_runs": scanned, "added": added, "next_before": next}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Decision {
    status: String,
    note: String,
}

async fn decide(
    state: &AppState,
    id: Uuid,
    actor: &str,
    request: Decision,
    relation: bool,
) -> AppResult<Value> {
    if !["confirmed", "rejected"].contains(&request.status.as_str()) {
        return Err(AppError::BadRequest("复核状态无效".into()));
    }
    let note = bounded_text(&request.note, "复核理由", 1, 2000)?;
    let mut tx = state.pool.begin().await?;
    let sql = if relation {
        "SELECT run_id, status FROM osint_relations WHERE id = $1 FOR UPDATE"
    } else {
        "SELECT run_id, status FROM osint_mentions WHERE id = $1 FOR UPDATE"
    };
    let row = sqlx::query(sql)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;
    let old: String = row.get("status");
    let run_id: Uuid = row.get("run_id");
    let sql = if relation {
        "UPDATE osint_relations SET status = $2 WHERE id = $1"
    } else {
        "UPDATE osint_mentions SET status = $2 WHERE id = $1"
    };
    sqlx::query(sql)
        .bind(id)
        .bind(&request.status)
        .execute(&mut *tx)
        .await?;
    audit(
        &mut tx,
        run_id,
        actor,
        if relation {
            "relation_reviewed"
        } else {
            "mention_reviewed"
        },
        json!({"id": id, "previous_status": old, "status": request.status, "note": note}),
    )
    .await?;
    tx.commit().await?;
    Ok(json!({"id": id, "status": request.status}))
}

async fn review_mention(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<Decision>,
) -> AppResult<Json<Value>> {
    let actor = auth::require_session(&state.pool, &headers).await?;
    Ok(Json(decide(&state, id, &actor, request, false).await?))
}

async fn review_relation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<Decision>,
) -> AppResult<Json<Value>> {
    let actor = auth::require_session(&state.pool, &headers).await?;
    Ok(Json(decide(&state, id, &actor, request, true).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RelationRequest {
    subject_id: Uuid,
    object_id: Uuid,
    predicate: String,
    article_id: Uuid,
    excerpt: String,
}

async fn add_relation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<RelationRequest>,
) -> AppResult<Json<Value>> {
    let actor = auth::require_session(&state.pool, &headers).await?;
    let data = report(&state, id).await?;
    let excerpt = evidence_excerpt(&data, request.article_id, &request.excerpt)?;
    let predicate = bounded_text(&request.predicate, "关系", 1, 80)?;
    if request.subject_id == request.object_id {
        return Err(AppError::BadRequest("请选择两个不同实体".into()));
    }
    let mut tx = state.pool.begin().await?;
    let count: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT entity_id) FROM osint_mentions WHERE run_id = $1
        AND status = 'confirmed' AND entity_id = ANY($2)",
    )
    .bind(id)
    .bind(vec![request.subject_id, request.object_id])
    .fetch_one(&mut *tx)
    .await?;
    if count != 2 {
        return Err(AppError::BadRequest(
            "关系两端须先在本次调查中确认标注".into(),
        ));
    }
    let relation_id: Uuid = sqlx::query_scalar("INSERT INTO osint_relations (run_id, article_id, subject_id, object_id, predicate, excerpt, created_by)
        VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id")
        .bind(id).bind(request.article_id).bind(request.subject_id).bind(request.object_id).bind(&predicate).bind(&excerpt).bind(&actor)
        .fetch_one(&mut *tx).await?;
    audit(
        &mut tx,
        id,
        &actor,
        "relation_added",
        json!({"id": relation_id, "subject_id": request.subject_id,
        "object_id": request.object_id, "predicate": predicate, "excerpt": excerpt}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({"id": relation_id})))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn alias_matching_preserves_unicode_and_respects_latin_boundaries() {
        assert!(alias_excerpt("The Taiwan report", "AI").is_none());
        assert!(alias_excerpt("OpenAI published a report", "openai").is_some());
        assert!(alias_excerpt("中国科学院发布了报告", "中国科学院").is_some());
        let text = "İstanbul officials met Acme representatives.";
        let excerpt = alias_excerpt(text, "acme").unwrap();
        assert!(text.contains(&excerpt));
        assert!(alias_excerpt("Unrelated text", "Elsewhere").is_none());
    }
    #[test]
    fn evidence_must_belong_to_immutable_run_material() {
        let id = Uuid::new_v4();
        let data = json!({"materials": [{"id": id, "content": "Acme opened an office in Paris."}]});
        assert!(evidence_excerpt(&data, id, "Acme opened an office").is_ok());
        assert!(evidence_excerpt(&data, Uuid::new_v4(), "Acme opened an office").is_err());
        assert!(evidence_excerpt(&data, id, "Acme secretly owns Paris").is_err());
        assert!(
            aliases("Acme", &["ACME".into(), "艾克米".into()])
                .unwrap()
                .len()
                == 2
        );
    }
}
