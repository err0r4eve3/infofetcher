//! End-to-end casebook checks reused by the isolated PostgreSQL research fixture.
use super::*;

async fn get(http: &reqwest::Client, base: &str, cookie: &str, path: &str) -> Value {
    http.get(format!("{base}{path}"))
        .header(header::COOKIE, cookie)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}
async fn post(
    http: &reqwest::Client,
    base: &str,
    cookie: &str,
    path: &str,
    body: Value,
    expected: u16,
) -> Value {
    let response = http
        .post(format!("{base}{path}"))
        .header(header::COOKIE, cookie)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let data: Value = response.json().await.unwrap();
    assert_eq!(status.as_u16(), expected, "{path}: {data}");
    data
}

pub(super) async fn exercise(
    pool: &sqlx::PgPool,
    http: &reqwest::Client,
    base: &str,
    cookie: &str,
    first: &Value,
    article: Uuid,
) {
    let run = first["run_id"].as_str().unwrap();
    let case = first["case_id"].as_str().unwrap();
    let story = first["event_id"].as_str().unwrap();
    for path in [
        format!("/api/v2/osint/runs/{run}"),
        format!("/api/v2/osint/cases/{case}"),
        "/api/v2/osint/entities".into(),
    ] {
        assert_eq!(
            http.get(format!("{base}{path}"))
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
    }
    let original = get(http, base, cookie, &format!("/api/v2/osint/runs/{run}")).await;
    assert_eq!(original["report"], *first);
    assert_eq!(
        original["report_sha256"],
        hex::encode(Sha256::digest(first.to_string().as_bytes()))
    );
    let history = get(
        http,
        base,
        cookie,
        &format!("/api/v2/events/{story}/osint/history"),
    )
    .await;
    assert_eq!(history["case"]["id"], case);
    assert_eq!(history["runs"][0]["id"], run);
    let review_path = format!("/api/v2/osint/runs/{run}/reviews");
    post(http, base, cookie, &review_path, json!({"target_kind":"fragment", "target_index":99999, "verdict":"confirmed", "note":"invalid"}), 400).await;
    post(http, base, cookie, &review_path, json!({"target_kind":"fragment", "target_index":0, "verdict":"confirmed", "note":"Checked original wording"}), 200).await;
    post(http, base, cookie, &review_path, json!({"target_kind":"fragment", "target_index":0, "verdict":"needs_review", "note":"Identity still requires confirmation"}), 200).await;
    let updated = get(http, base, cookie, &format!("/api/v2/osint/runs/{run}")).await;
    assert_eq!(updated["reviews"].as_array().unwrap().len(), 2);
    assert_eq!(updated["reviews"][0]["verdict"], "needs_review");
    assert_eq!(
        updated["report"], original["report"],
        "review must not rewrite the snapshot"
    );
    assert_eq!(updated["report_sha256"], original["report_sha256"]);
    let entity_path = format!("/api/v2/osint/runs/{run}/entities");
    post(
        http,
        base,
        cookie,
        &entity_path,
        json!({"name":"Acme", "kind":"organization", "article_id":article,
        "excerpt":"Acme invented evidence not in the source"}),
        400,
    )
    .await;
    let excerpt = "Acme opened an office in Paris.";
    let acme = post(
        http,
        base,
        cookie,
        &entity_path,
        json!({"name":"Acme", "kind":"organization", "aliases":["艾克米"],
        "article_id":article, "excerpt":excerpt}),
        200,
    )
    .await;
    let paris = post(
        http,
        base,
        cookie,
        &entity_path,
        json!({"name":"Paris", "kind":"location", "aliases":["巴黎"],
        "article_id":article, "excerpt":excerpt}),
        200,
    )
    .await;
    let entity = acme["entity_id"].as_str().unwrap();
    let catalog = get(http, base, cookie, "/api/v2/osint/entities?q=Acme").await;
    assert!(
        catalog["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["id"] == entity)
    );
    let relation = post(http, base, cookie, &format!("/api/v2/osint/runs/{run}/relations"), json!({"subject_id":entity,
        "object_id":paris["entity_id"], "predicate":"设有办公室", "article_id":article, "excerpt":excerpt}), 200).await;
    post(
        http,
        base,
        cookie,
        &format!(
            "/api/v2/osint/relations/{}/review",
            relation["id"].as_str().unwrap()
        ),
        json!({"status":"rejected", "note":"The scope needs qualification"}),
        200,
    )
    .await;

    // A separate event with a Chinese alias must be a candidate, not a confirmed identity.
    let other_story: Uuid = sqlx::query_scalar("INSERT INTO story_clusters (representative_title, story_key) VALUES ('Alias background', $1) RETURNING id")
        .bind(Uuid::new_v4().to_string()).fetch_one(pool).await.unwrap();
    let feed: Uuid = sqlx::query_scalar("SELECT feed_id FROM articles WHERE id = $1")
        .bind(article)
        .fetch_one(pool)
        .await
        .unwrap();
    let other_article: Uuid = sqlx::query_scalar("INSERT INTO articles (feed_id, story_id, guid, url_hash, title, url, canonical_url, content_hash, content)
        VALUES ($1, $2, $3, $3, 'Alias background', 'https://fixture.example/alias', 'https://fixture.example/alias', $3, $4) RETURNING id")
        .bind(feed).bind(other_story).bind(Uuid::new_v4().to_string())
        .bind("艾克米在巴黎宣布新的合作计划，具体合作范围仍待后续文件说明。".repeat(10)).fetch_one(pool).await.unwrap();
    let second = post(
        http,
        base,
        cookie,
        &format!("/api/v2/events/{other_story}/osint/research"),
        json!({}),
        200,
    )
    .await;
    let second_run = second["run_id"].as_str().unwrap();
    let scan_path = format!("/api/v2/osint/entities/{entity}/scan");
    post(
        http,
        base,
        cookie,
        &scan_path,
        json!({"before": Uuid::new_v4()}),
        404,
    )
    .await;
    // Cross the 20-run page boundary. Each stored report retains its own ID/hash.
    for index in 0..21 {
        let mut snapshot = second.clone();
        let extra = Uuid::new_v4();
        let time = Utc::now() + chrono::Duration::seconds(index);
        snapshot["run_id"] = json!(extra);
        snapshot["generated_at"] = json!(time);
        sqlx::query(
            "INSERT INTO osint_runs (id, case_id, created_at, created_by, report, report_sha256)
            VALUES ($1, $2, $3, 'pagination-fixture', $4, $5)",
        )
        .bind(extra)
        .bind(Uuid::parse_str(second["case_id"].as_str().unwrap()).unwrap())
        .bind(time)
        .bind(&snapshot)
        .bind(hex::encode(Sha256::digest(snapshot.to_string().as_bytes())))
        .execute(pool)
        .await
        .unwrap();
    }
    let mut cursor = Value::Null;
    let mut pages = 0;
    let mut added = 0;
    loop {
        let scan = post(
            http,
            base,
            cookie,
            &scan_path,
            json!({"before": cursor}),
            200,
        )
        .await;
        assert!(scan["scanned_runs"].as_u64().unwrap() <= 20);
        added += scan["added"].as_i64().unwrap();
        pages += 1;
        cursor = scan["next_before"].clone();
        if cursor.is_null() {
            break;
        }
        assert!(pages < 100, "cursor must make progress");
    }
    assert!(added > 0 && pages >= 2);
    let profile = get(
        http,
        base,
        cookie,
        &format!("/api/v2/osint/entities/{entity}"),
    )
    .await;
    let suggestion = profile["mentions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["run_id"] == second_run)
        .unwrap();
    assert_eq!(suggestion["status"], "suggested");
    assert_eq!(suggestion["matched_alias"], "艾克米");
    let mention_id = suggestion["id"].as_str().unwrap();
    post(
        http,
        base,
        cookie,
        &format!("/api/v2/osint/mentions/{mention_id}/review"),
        json!({"status":"rejected", "note":"Same alias, identity not established"}),
        200,
    )
    .await;
    let scan = post(http, base, cookie, &scan_path, json!({}), 200).await;
    assert_eq!(scan["added"], 0, "repeat scans must be idempotent");
    let profile = get(
        http,
        base,
        cookie,
        &format!("/api/v2/osint/entities/{entity}"),
    )
    .await;
    assert_eq!(
        profile["mentions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["id"] == mention_id)
            .unwrap()["status"],
        "rejected"
    );

    // Capture body and source IDs survive both reader article and event deletion.
    let body = b"<html><body>Preserved original evidence</body></html>";
    let capture: Uuid = sqlx::query_scalar("INSERT INTO osint_captures (case_id, article_id, story_id, captured_at, sha256, body, details)
        VALUES ($1, $2, $3, now(), $4, $5, '{\"article_title\":\"Alias background\",\"http_status\":200}') RETURNING id")
        .bind(Uuid::parse_str(second["case_id"].as_str().unwrap()).unwrap()).bind(other_article).bind(other_story)
        .bind(hex::encode(Sha256::digest(body))).bind(body.as_slice()).fetch_one(pool).await.unwrap();
    let third = post(
        http,
        base,
        cookie,
        &format!("/api/v2/events/{other_story}/osint/research"),
        json!({}),
        200,
    )
    .await;
    assert_eq!(third["captures"][0]["id"], capture.to_string());
    assert_ne!(third["run_id"], second["run_id"]);
    assert_eq!(third["case_id"], second["case_id"]);
    sqlx::query("DELETE FROM articles WHERE id = $1")
        .bind(other_article)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM story_clusters WHERE id = $1")
        .bind(other_story)
        .execute(pool)
        .await
        .unwrap();
    let retained = get(
        http,
        base,
        cookie,
        &format!(
            "/api/v2/osint/cases/{}",
            second["case_id"].as_str().unwrap()
        ),
    )
    .await;
    assert_eq!(retained["case"]["event_available"], false);
    assert_eq!(retained["runs"].as_array().unwrap().len(), 23);
    assert_eq!(retained["captures"][0]["id"], capture.to_string());
    let retained_run = get(
        http,
        base,
        cookie,
        &format!("/api/v2/osint/runs/{second_run}"),
    )
    .await;
    assert_eq!(retained_run["report"], second);
    assert_eq!(retained_run["mentions"][0]["status"], "rejected");
    let raw = get(
        http,
        base,
        cookie,
        &format!("/api/v2/osint/captures/{capture}/export"),
    )
    .await;
    assert_eq!(
        hex::decode(raw["body_hex"].as_str().unwrap()).unwrap(),
        body
    );
    let export = get(
        http,
        base,
        cookie,
        &format!("/api/v2/osint/runs/{run}/export"),
    )
    .await;
    assert_eq!(export["run"]["reviews"].as_array().unwrap().len(), 2);
    assert_eq!(export["run"]["relations"][0]["status"], "rejected");
    assert!(!export["run"]["entity_audit"].as_array().unwrap().is_empty());
    let listed = get(http, base, cookie, "/api/v2/osint/cases?q=Alias").await;
    assert!(
        listed["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == second["case_id"])
    );
}
