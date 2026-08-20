use sqlx::{Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    feeds::excerpt,
    models::{
        KeywordActionResponse, KeywordMatch, KeywordMatchActionRequest, KeywordRule,
        KeywordRuleRequest, KeywordScanResponse,
    },
    push::{self, PushMessage},
    state::AppState,
    worker,
};

#[derive(Debug, Clone)]
pub struct KeywordMatcher {
    pub keywords: Vec<String>,
    pub match_title: bool,
    pub match_content: bool,
    pub case_sensitive: bool,
}

#[derive(Debug, Clone)]
pub struct KeywordArticleText<'a> {
    pub title: &'a str,
    pub content: &'a str,
    pub extracted_content: &'a str,
}

pub fn normalize_keyword_input(input: &str) -> Vec<String> {
    let mut keywords = Vec::new();
    let mut seen = Vec::<String>::new();

    for keyword in input
        .split([',', '，', '\n', '\r', ';', '；'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let key = keyword.to_lowercase();
        if seen.iter().any(|item| item == &key) {
            continue;
        }
        seen.push(key);
        keywords.push(keyword.to_string());
    }

    keywords
}

pub fn detect_keywords(rule: &KeywordMatcher, article: &KeywordArticleText<'_>) -> Vec<String> {
    if rule.keywords.is_empty() || (!rule.match_title && !rule.match_content) {
        return Vec::new();
    }

    let body = if article.extracted_content.trim().is_empty() {
        article.content
    } else {
        article.extracted_content
    };
    let mut haystack = String::new();
    if rule.match_title {
        haystack.push_str(article.title);
        haystack.push('\n');
    }
    if rule.match_content {
        haystack.push_str(body);
    }

    if rule.case_sensitive {
        return rule
            .keywords
            .iter()
            .filter(|keyword| !keyword.is_empty() && haystack.contains(keyword.as_str()))
            .cloned()
            .collect();
    }

    let lower_haystack = haystack.to_lowercase();
    rule.keywords
        .iter()
        .filter(|keyword| {
            let needle = keyword.to_lowercase();
            !needle.is_empty() && lower_haystack.contains(needle.as_str())
        })
        .cloned()
        .collect()
}

#[derive(Debug, Clone)]
struct KeywordArticleRow {
    id: Uuid,
    feed_id: Uuid,
    folder_id: Option<Uuid>,
    title: String,
    content: String,
    extracted_content: String,
}

#[derive(Debug)]
struct MatchWriteResult {
    id: Uuid,
    inserted: bool,
}

#[derive(Debug, Default)]
struct KeywordActionStats {
    queued_summaries: i64,
    telegram_sent: i64,
    telegram_failed: i64,
}

pub async fn list_rules(state: &AppState) -> AppResult<Vec<KeywordRule>> {
    let sql = rule_select_sql(None);
    let rows = sqlx::query(&sql).fetch_all(&state.pool).await?;
    Ok(rows.into_iter().map(rule_from_row).collect())
}

pub async fn create_rule(state: &AppState, body: KeywordRuleRequest) -> AppResult<KeywordRule> {
    let validated = validate_rule(body)?;
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query(
        "INSERT INTO keyword_rules
          (name, keywords, match_title, match_content, case_sensitive, auto_summary, telegram_enabled, enabled)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         RETURNING id",
    )
    .bind(&validated.name)
    .bind(&validated.keywords)
    .bind(validated.match_title)
    .bind(validated.match_content)
    .bind(validated.case_sensitive)
    .bind(validated.auto_summary)
    .bind(validated.telegram_enabled)
    .bind(validated.enabled)
    .fetch_one(&mut *tx)
    .await?;
    let rule_id = row.get("id");
    save_rule_targets(&mut tx, rule_id, &validated.feed_ids, &validated.folder_ids).await?;
    tx.commit().await?;
    get_rule(state, rule_id).await
}

pub async fn update_rule(
    state: &AppState,
    rule_id: Uuid,
    body: KeywordRuleRequest,
) -> AppResult<KeywordRule> {
    let validated = validate_rule(body)?;
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query(
        "UPDATE keyword_rules
         SET name = $2,
             keywords = $3,
             match_title = $4,
             match_content = $5,
             case_sensitive = $6,
             auto_summary = $7,
             telegram_enabled = $8,
             enabled = $9,
             updated_at = now()
         WHERE id = $1
         RETURNING id",
    )
    .bind(rule_id)
    .bind(&validated.name)
    .bind(&validated.keywords)
    .bind(validated.match_title)
    .bind(validated.match_content)
    .bind(validated.case_sensitive)
    .bind(validated.auto_summary)
    .bind(validated.telegram_enabled)
    .bind(validated.enabled)
    .fetch_optional(&mut *tx)
    .await?;
    if row.is_none() {
        return Err(AppError::NotFound);
    }
    sqlx::query("DELETE FROM keyword_rule_feeds WHERE rule_id = $1")
        .bind(rule_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM keyword_rule_folders WHERE rule_id = $1")
        .bind(rule_id)
        .execute(&mut *tx)
        .await?;
    save_rule_targets(&mut tx, rule_id, &validated.feed_ids, &validated.folder_ids).await?;
    tx.commit().await?;
    get_rule(state, rule_id).await
}

pub async fn delete_rule(state: &AppState, rule_id: Uuid) -> AppResult<()> {
    let result = sqlx::query("DELETE FROM keyword_rules WHERE id = $1")
        .bind(rule_id)
        .execute(&state.pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

pub async fn scan_rule(
    state: &AppState,
    rule_id: Uuid,
    limit: Option<i64>,
) -> AppResult<KeywordScanResponse> {
    let rule = get_rule(state, rule_id).await?;
    let rows = query_articles_for_rule(state, &rule, limit.unwrap_or(200).clamp(1, 1000)).await?;
    let matcher = matcher_from_rule(&rule);
    let mut response = KeywordScanResponse {
        scanned: rows.len() as i64,
        matched: 0,
        new_matches: 0,
        queued_summaries: 0,
        telegram_sent: 0,
        telegram_failed: 0,
    };

    for article in rows {
        let matched_keywords = detect_keywords(
            &matcher,
            &KeywordArticleText {
                title: &article.title,
                content: &article.content,
                extracted_content: &article.extracted_content,
            },
        );
        if matched_keywords.is_empty() {
            continue;
        }
        response.matched += 1;
        let write = record_keyword_match(state, &rule, article.id, &matched_keywords).await?;
        if write.inserted {
            response.new_matches += 1;
        }
        let stats = run_auto_actions(state, &rule, &write).await?;
        response.queued_summaries += stats.queued_summaries;
        response.telegram_sent += stats.telegram_sent;
        response.telegram_failed += stats.telegram_failed;
    }

    Ok(response)
}

pub async fn list_matches(
    state: &AppState,
    rule_id: Option<Uuid>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> AppResult<Vec<KeywordMatch>> {
    let mut qb = QueryBuilder::<Postgres>::new(
        "SELECT km.id, km.rule_id, kr.name AS rule_name, km.article_id,
                a.feed_id, f.title AS feed_title, a.title AS article_title, a.url AS article_url,
                a.extracted_content, a.content, a.published_at, a.fetched_at,
                km.matched_keywords, s.status AS summary_status,
                km.telegram_status, km.telegram_error, km.created_at
         FROM keyword_matches km
         JOIN keyword_rules kr ON kr.id = km.rule_id
         JOIN articles a ON a.id = km.article_id
         JOIN feeds f ON f.id = a.feed_id
         LEFT JOIN summaries s ON s.article_id = a.id
         WHERE true",
    );
    if let Some(rule_id) = rule_id {
        qb.push(" AND km.rule_id = ").push_bind(rule_id);
    }
    qb.push(" ORDER BY km.created_at DESC LIMIT ")
        .push_bind(limit.unwrap_or(80).clamp(1, 200))
        .push(" OFFSET ")
        .push_bind(offset.unwrap_or(0).max(0));
    let rows = qb.build().fetch_all(&state.pool).await?;
    Ok(rows.into_iter().map(match_from_row).collect())
}

pub async fn run_match_actions(
    state: &AppState,
    request: KeywordMatchActionRequest,
) -> AppResult<KeywordActionResponse> {
    if request.match_ids.is_empty() {
        return Err(AppError::BadRequest("请选择至少一个关键词命中".to_string()));
    }
    if !request.summarize && !request.telegram {
        return Err(AppError::BadRequest(
            "请选择 AI 总结或 Telegram 推送".to_string(),
        ));
    }
    let match_ids = dedupe_uuids(request.match_ids);
    let rows = sqlx::query("SELECT id, article_id FROM keyword_matches WHERE id = ANY($1)")
        .bind(&match_ids)
        .fetch_all(&state.pool)
        .await?;
    let mut response = KeywordActionResponse {
        queued_summaries: 0,
        telegram_sent: 0,
        telegram_failed: 0,
    };

    for row in rows {
        let match_id = row.get::<Uuid, _>("id");
        if request.summarize {
            let article_id = row.get::<Uuid, _>("article_id");
            let queued = worker::queue_summary_for_article(state, article_id).await?;
            mark_summary_queued(state, match_id).await?;
            if queued {
                response.queued_summaries += 1;
            }
        }
        if request.telegram {
            match send_telegram_for_match(state, match_id).await {
                Ok(()) => response.telegram_sent += 1,
                Err(_) => response.telegram_failed += 1,
            }
        }
    }

    Ok(response)
}

pub async fn process_article_keyword_rules(state: &AppState, article_id: Uuid) -> AppResult<i64> {
    let article = query_article_by_id(state, article_id).await?;
    let rules = list_rules(state).await?;
    let mut new_matches = 0;

    for rule in rules.into_iter().filter(|rule| rule.enabled) {
        if !rule_applies_to_article(&rule, article.feed_id, article.folder_id) {
            continue;
        }
        let matched_keywords = detect_keywords(
            &matcher_from_rule(&rule),
            &KeywordArticleText {
                title: &article.title,
                content: &article.content,
                extracted_content: &article.extracted_content,
            },
        );
        if matched_keywords.is_empty() {
            continue;
        }
        let write = record_keyword_match(state, &rule, article.id, &matched_keywords).await?;
        if write.inserted {
            new_matches += 1;
        }
        run_auto_actions(state, &rule, &write).await?;
    }

    Ok(new_matches)
}

async fn get_rule(state: &AppState, rule_id: Uuid) -> AppResult<KeywordRule> {
    let sql = rule_select_sql(Some("WHERE r.id = $1"));
    let row = sqlx::query(&sql)
        .bind(rule_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(rule_from_row(row))
}

fn rule_select_sql(where_clause: Option<&str>) -> String {
    format!(
        "SELECT r.id, r.name, r.keywords, r.match_title, r.match_content, r.case_sensitive,
                r.auto_summary, r.telegram_enabled, r.enabled, r.created_at, r.updated_at,
                COALESCE(array_agg(DISTINCT rf.feed_id) FILTER (WHERE rf.feed_id IS NOT NULL), '{{}}'::uuid[]) AS feed_ids,
                COALESCE(array_agg(DISTINCT rfo.folder_id) FILTER (WHERE rfo.folder_id IS NOT NULL), '{{}}'::uuid[]) AS folder_ids,
                count(DISTINCT km.id)::bigint AS match_count
         FROM keyword_rules r
         LEFT JOIN keyword_rule_feeds rf ON rf.rule_id = r.id
         LEFT JOIN keyword_rule_folders rfo ON rfo.rule_id = r.id
         LEFT JOIN keyword_matches km ON km.rule_id = r.id
         {}
         GROUP BY r.id
         ORDER BY r.created_at DESC",
        where_clause.unwrap_or("")
    )
}

fn rule_from_row(row: sqlx::postgres::PgRow) -> KeywordRule {
    KeywordRule {
        id: row.get("id"),
        name: row.get("name"),
        keywords: row.get("keywords"),
        match_title: row.get("match_title"),
        match_content: row.get("match_content"),
        case_sensitive: row.get("case_sensitive"),
        auto_summary: row.get("auto_summary"),
        telegram_enabled: row.get("telegram_enabled"),
        enabled: row.get("enabled"),
        feed_ids: row.get("feed_ids"),
        folder_ids: row.get("folder_ids"),
        match_count: row.get("match_count"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

async fn save_rule_targets(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    rule_id: Uuid,
    feed_ids: &[Uuid],
    folder_ids: &[Uuid],
) -> AppResult<()> {
    for feed_id in feed_ids {
        sqlx::query("INSERT INTO keyword_rule_feeds (rule_id, feed_id) VALUES ($1, $2)")
            .bind(rule_id)
            .bind(feed_id)
            .execute(&mut **tx)
            .await?;
    }
    for folder_id in folder_ids {
        sqlx::query("INSERT INTO keyword_rule_folders (rule_id, folder_id) VALUES ($1, $2)")
            .bind(rule_id)
            .bind(folder_id)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

fn validate_rule(body: KeywordRuleRequest) -> AppResult<KeywordRuleRequest> {
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::BadRequest("规则名称不能为空".to_string()));
    }
    if !body.match_title && !body.match_content {
        return Err(AppError::BadRequest(
            "至少需要检测标题或正文之一".to_string(),
        ));
    }
    let keywords = normalize_keyword_input(&body.keywords.join("\n"));
    if keywords.is_empty() {
        return Err(AppError::BadRequest("至少需要一个关键词".to_string()));
    }
    Ok(KeywordRuleRequest {
        name,
        keywords,
        match_title: body.match_title,
        match_content: body.match_content,
        case_sensitive: body.case_sensitive,
        auto_summary: body.auto_summary,
        telegram_enabled: body.telegram_enabled,
        enabled: body.enabled,
        feed_ids: dedupe_uuids(body.feed_ids),
        folder_ids: dedupe_uuids(body.folder_ids),
    })
}

fn dedupe_uuids(values: Vec<Uuid>) -> Vec<Uuid> {
    let mut deduped = Vec::new();
    for value in values {
        if !deduped.contains(&value) {
            deduped.push(value);
        }
    }
    deduped
}

fn matcher_from_rule(rule: &KeywordRule) -> KeywordMatcher {
    KeywordMatcher {
        keywords: rule.keywords.clone(),
        match_title: rule.match_title,
        match_content: rule.match_content,
        case_sensitive: rule.case_sensitive,
    }
}

async fn query_articles_for_rule(
    state: &AppState,
    rule: &KeywordRule,
    limit: i64,
) -> AppResult<Vec<KeywordArticleRow>> {
    let mut qb = QueryBuilder::<Postgres>::new(
        "SELECT a.id, a.feed_id, f.folder_id, a.title,
                a.content, a.extracted_content
         FROM articles a
         JOIN feeds f ON f.id = a.feed_id
         WHERE true",
    );
    push_rule_target_filter(&mut qb, rule);
    qb.push(" ORDER BY COALESCE(a.published_at, a.fetched_at) DESC LIMIT ")
        .push_bind(limit);
    let rows = qb.build().fetch_all(&state.pool).await?;
    Ok(rows.into_iter().map(article_row_from_row).collect())
}

async fn query_article_by_id(state: &AppState, article_id: Uuid) -> AppResult<KeywordArticleRow> {
    let row = sqlx::query(
        "SELECT a.id, a.feed_id, f.folder_id, a.title,
                a.content, a.extracted_content
         FROM articles a
         JOIN feeds f ON f.id = a.feed_id
         WHERE a.id = $1",
    )
    .bind(article_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(article_row_from_row(row))
}

fn push_rule_target_filter(qb: &mut QueryBuilder<'_, Postgres>, rule: &KeywordRule) {
    if rule.feed_ids.is_empty() && rule.folder_ids.is_empty() {
        return;
    }
    qb.push(" AND (false");
    if !rule.feed_ids.is_empty() {
        qb.push(" OR a.feed_id = ANY(")
            .push_bind(rule.feed_ids.clone())
            .push(")");
    }
    if !rule.folder_ids.is_empty() {
        qb.push(" OR f.folder_id = ANY(")
            .push_bind(rule.folder_ids.clone())
            .push(")");
    }
    qb.push(")");
}

fn article_row_from_row(row: sqlx::postgres::PgRow) -> KeywordArticleRow {
    KeywordArticleRow {
        id: row.get("id"),
        feed_id: row.get("feed_id"),
        folder_id: row.get("folder_id"),
        title: row.get("title"),
        content: row.get("content"),
        extracted_content: row.get("extracted_content"),
    }
}

fn rule_applies_to_article(rule: &KeywordRule, feed_id: Uuid, folder_id: Option<Uuid>) -> bool {
    if rule.feed_ids.is_empty() && rule.folder_ids.is_empty() {
        return true;
    }
    rule.feed_ids.contains(&feed_id)
        || folder_id.is_some_and(|folder_id| rule.folder_ids.contains(&folder_id))
}

async fn record_keyword_match(
    state: &AppState,
    rule: &KeywordRule,
    article_id: Uuid,
    matched_keywords: &[String],
) -> AppResult<MatchWriteResult> {
    if let Some(row) = sqlx::query(
        "INSERT INTO keyword_matches (rule_id, article_id, matched_keywords)
         VALUES ($1, $2, $3)
         ON CONFLICT (rule_id, article_id) DO NOTHING
         RETURNING id",
    )
    .bind(rule.id)
    .bind(article_id)
    .bind(matched_keywords)
    .fetch_optional(&state.pool)
    .await?
    {
        return Ok(MatchWriteResult {
            id: row.get("id"),
            inserted: true,
        });
    }

    let row = sqlx::query(
        "UPDATE keyword_matches
         SET matched_keywords = $3, updated_at = now()
         WHERE rule_id = $1 AND article_id = $2
         RETURNING id",
    )
    .bind(rule.id)
    .bind(article_id)
    .bind(matched_keywords)
    .fetch_one(&state.pool)
    .await?;
    Ok(MatchWriteResult {
        id: row.get("id"),
        inserted: false,
    })
}

async fn run_auto_actions(
    state: &AppState,
    rule: &KeywordRule,
    write: &MatchWriteResult,
) -> AppResult<KeywordActionStats> {
    let mut stats = KeywordActionStats::default();
    let match_state = sqlx::query(
        "SELECT article_id, summary_queued, telegram_status
         FROM keyword_matches WHERE id = $1",
    )
    .bind(write.id)
    .fetch_one(&state.pool)
    .await?;
    if rule.auto_summary && !match_state.get::<bool, _>("summary_queued") {
        let queued =
            worker::queue_summary_for_article(state, match_state.get::<Uuid, _>("article_id"))
                .await?;
        mark_summary_queued(state, write.id).await?;
        if queued {
            stats.queued_summaries += 1;
        }
    }
    if rule.telegram_enabled && match_state.get::<String, _>("telegram_status") != "sent" {
        match send_telegram_for_match(state, write.id).await {
            Ok(()) => stats.telegram_sent += 1,
            Err(_) => stats.telegram_failed += 1,
        }
    }
    Ok(stats)
}

async fn mark_summary_queued(state: &AppState, match_id: Uuid) -> AppResult<()> {
    sqlx::query(
        "UPDATE keyword_matches SET summary_queued = true, updated_at = now() WHERE id = $1",
    )
    .bind(match_id)
    .execute(&state.pool)
    .await?;
    Ok(())
}

async fn send_telegram_for_match(state: &AppState, match_id: Uuid) -> AppResult<()> {
    let row = sqlx::query(
        "SELECT kr.name AS rule_name, km.matched_keywords,
                a.title AS article_title, a.url AS article_url,
                f.title AS feed_title, s.summary
         FROM keyword_matches km
         JOIN keyword_rules kr ON kr.id = km.rule_id
         JOIN articles a ON a.id = km.article_id
         JOIN feeds f ON f.id = a.feed_id
         LEFT JOIN summaries s ON s.article_id = a.id
         WHERE km.id = $1",
    )
    .bind(match_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    let matched_keywords = row.get::<Vec<String>, _>("matched_keywords");
    let rule_name = row.get::<String, _>("rule_name");
    let article_title = row.get::<String, _>("article_title");
    let feed_title = row.get::<String, _>("feed_title");
    let summary = row.get::<Option<String>, _>("summary");
    let article_url = row.get::<String, _>("article_url");
    let message = push::build_telegram_message(&PushMessage {
        rule_name: &rule_name,
        matched_keywords: &matched_keywords,
        article_title: &article_title,
        feed_title: &feed_title,
        summary: summary.as_deref(),
        article_url: &article_url,
    });

    match push::send_telegram(state.config.as_ref(), &state.http, &message).await {
        Ok(()) => {
            sqlx::query(
                "UPDATE keyword_matches
                 SET telegram_status = 'sent', telegram_error = NULL, updated_at = now()
                 WHERE id = $1",
            )
            .bind(match_id)
            .execute(&state.pool)
            .await?;
            Ok(())
        }
        Err(err) => {
            sqlx::query(
                "UPDATE keyword_matches
                 SET telegram_status = 'failed', telegram_error = $2, updated_at = now()
                 WHERE id = $1",
            )
            .bind(match_id)
            .bind(err.to_string())
            .execute(&state.pool)
            .await?;
            Err(err)
        }
    }
}

fn match_from_row(row: sqlx::postgres::PgRow) -> KeywordMatch {
    let extracted = row.get::<String, _>("extracted_content");
    let content = if extracted.trim().is_empty() {
        row.get::<String, _>("content")
    } else {
        extracted
    };
    KeywordMatch {
        id: row.get("id"),
        rule_id: row.get("rule_id"),
        rule_name: row.get("rule_name"),
        article_id: row.get("article_id"),
        feed_id: row.get("feed_id"),
        feed_title: row.get("feed_title"),
        article_title: row.get("article_title"),
        article_url: row.get("article_url"),
        excerpt: excerpt(&content, 160),
        published_at: row.get("published_at"),
        fetched_at: row.get("fetched_at"),
        matched_keywords: row.get("matched_keywords"),
        summary_status: row.get("summary_status"),
        telegram_status: row.get("telegram_status"),
        telegram_error: row.get("telegram_error"),
        created_at: row.get("created_at"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_multiline_comma_keywords_and_deduplicates_case_insensitively() {
        let keywords = normalize_keyword_input(" AI, Rust\nAI，rust\n  Telegram  ");

        assert_eq!(keywords, vec!["AI", "Rust", "Telegram"]);
    }

    #[test]
    fn detects_keywords_from_enabled_article_fields() {
        let rule = KeywordMatcher {
            keywords: vec!["Rust".to_string(), "LLM".to_string()],
            match_title: true,
            match_content: false,
            case_sensitive: false,
        };
        let article = KeywordArticleText {
            title: "Rust 发布新版本",
            content: "正文提到了 LLM，但当前规则不检测正文。",
            extracted_content: "",
        };

        assert_eq!(detect_keywords(&rule, &article), vec!["Rust"]);
    }

    #[test]
    fn respects_case_sensitive_matching() {
        let rule = KeywordMatcher {
            keywords: vec!["AI".to_string()],
            match_title: true,
            match_content: true,
            case_sensitive: true,
        };
        let article = KeywordArticleText {
            title: "ai 工具更新",
            content: "大写 AI 出现在正文",
            extracted_content: "",
        };

        assert_eq!(detect_keywords(&rule, &article), vec!["AI"]);
    }
}
