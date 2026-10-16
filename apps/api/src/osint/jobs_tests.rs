//! Real HTTP + PostgreSQL checks, invoked by the disposable native integration test.
use super::*;

pub(super) async fn exercise(state: &AppState, http: &reqwest::Client, base: &str, cookie: &str) {
    let pool = &state.pool;
    let story:Uuid=sqlx::query_scalar("INSERT INTO story_clusters(representative_title,story_key) VALUES ('Document investigation',$1) RETURNING id").bind(Uuid::new_v4().to_string()).fetch_one(pool).await.unwrap();
    let upload = format!("{base}/api/v2/events/{story}/osint/documents?filename=record.txt");
    assert_eq!(
        http.post(&upload)
            .body("local evidence")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        http.post(&upload)
            .header(header::COOKIE, cookie)
            .body(vec![0, 1, 2])
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let bytes =
        "Acme opened an office in Paris. The original record lists 125 samples. 中文原始文件。";
    let doc: Value = http
        .post(&upload)
        .header(header::COOKIE, cookie)
        .body(bytes)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let case_id = doc["case_id"].as_str().unwrap();
    let doc_id = doc["id"].as_str().unwrap();
    let duplicate: Value = http
        .post(&upload)
        .header(header::COOKIE, cookie)
        .body(bytes)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(duplicate["duplicate"], true);
    assert_eq!(duplicate["id"], doc["id"]);
    let queue = format!("{base}/api/v2/osint/cases/{case_id}/jobs");
    let queued: Value = http
        .post(&queue)
        .header(header::COOKIE, cookie)
        .json(&json!({"query":"Acme", "mode":"local"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let again: Value = http
        .post(&queue)
        .header(header::COOKIE, cookie)
        .json(&json!({"query":"changed", "mode":"local"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(again["id"], queued["id"]);
    assert_eq!(again["existing"], true);
    assert!(jobs::run_once(state).await.unwrap()); // Text extraction.
    assert!(jobs::run_once(state).await.unwrap()); // Frozen before the extraction.
    let first: Value = http
        .get(format!(
            "{base}/api/v2/osint/runs/{}",
            queued["id"].as_str().unwrap()
        ))
        .header(header::COOKIE, cookie)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first["report"]["materials"].as_array().unwrap().len(), 0);
    assert!(
        first["report"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("尚未成功提取"))
    );
    let queued: Value = http
        .post(&queue)
        .header(header::COOKIE, cookie)
        .json(&json!({"query":"Acme", "mode":"local"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let job_id: Uuid = queued["id"].as_str().unwrap().parse().unwrap();
    // Simulate a process that died after claiming the durable job.
    sqlx::query("UPDATE osint_jobs SET status='running',attempts=1,lease_token=$2,lease_until=now()-interval '1 second' WHERE id=$1").bind(job_id).bind(Uuid::new_v4()).execute(pool).await.unwrap();
    sqlx::query("DELETE FROM story_clusters WHERE id=$1")
        .bind(story)
        .execute(pool)
        .await
        .unwrap();
    assert!(jobs::run_once(state).await.unwrap());
    let report: Value = http
        .get(format!("{base}/api/v2/osint/runs/{job_id}"))
        .header(header::COOKIE, cookie)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(report["report"]["materials"][0]["document"]["id"], doc_id);
    assert!(
        report["report"]["materials"][0]["content"]
            .as_str()
            .unwrap()
            .contains("[第 1 页]")
    );
    assert_eq!(report["report"]["dependency_groups"], 1);
    assert_eq!(
        report["report_sha256"],
        hex::encode(Sha256::digest(report["report"].to_string().as_bytes()))
    );
    assert!(!jobs::run_once(state).await.unwrap());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM osint_runs WHERE id=$1")
        .bind(job_id)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let list: Value = http
        .get(&queue)
        .header(header::COOKIE, cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        list["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|job| job.get("input").is_none() && job.get("lease_token").is_none())
    );
    assert_eq!(list["items"][0]["attempts"], 2);
    let original = http
        .get(format!("{base}/api/v2/osint/documents/{doc_id}/original"))
        .header(header::COOKIE, cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(
        original.headers()[header::CONTENT_TYPE],
        "application/octet-stream"
    );
    assert_eq!(
        original.headers()[header::X_CONTENT_TYPE_OPTIONS],
        "nosniff"
    );
    assert_eq!(original.bytes().await.unwrap().as_ref(), bytes.as_bytes());
    // Cancellation retains the original; explicit retry processes that same job.
    let upload = format!("{base}/api/v2/osint/cases/{case_id}/documents?filename=second.txt");
    let second: Value = http
        .post(&upload)
        .header(header::COOKIE, cookie)
        .body("Second local evidence record.")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let second_job = second["job_id"].as_str().unwrap();
    http.post(format!("{base}/api/v2/osint/jobs/{second_job}/cancel"))
        .header(header::COOKIE, cookie)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert!(!jobs::run_once(state).await.unwrap());
    http.post(format!("{base}/api/v2/osint/jobs/{second_job}/retry"))
        .header(header::COOKIE, cookie)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert!(jobs::run_once(state).await.unwrap());
    assert_eq!(
        http.post(format!("{base}/api/v2/osint/jobs/{second_job}/retry"))
            .header(header::COOKIE, cookie)
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    // Empty text becomes an explicit failed job with downloadable original bytes.
    let blank: Value = http
        .post(&upload)
        .header(header::COOKIE, cookie)
        .body("  \n  ")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(jobs::run_once(state).await.unwrap());
    let blank_id: Uuid = blank["id"].as_str().unwrap().parse().unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM osint_documents WHERE id=$1")
        .bind(blank_id)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(status, "failed");
    let blank_job: Uuid = blank["job_id"].as_str().unwrap().parse().unwrap();
    sqlx::query("UPDATE osint_jobs SET status='running',attempts=3,lease_token=$2,lease_until=now()-interval '1 second' WHERE id=$1").bind(blank_job).bind(Uuid::new_v4()).execute(pool).await.unwrap();
    assert!(!jobs::run_once(state).await.unwrap());
    let status: String = sqlx::query_scalar("SELECT status FROM osint_jobs WHERE id=$1")
        .bind(blank_job)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(status, "failed");
    // Archived cases can still investigate imported documents; unknown publishers
    // remain one conservative group even across different files.
    let queued: Value = http
        .post(&queue)
        .header(header::COOKIE, cookie)
        .json(&json!({"query":"Acme", "mode":"local"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let (a, b) = tokio::join!(jobs::run_once(state), jobs::run_once(state));
    assert_ne!(
        a.unwrap(),
        b.unwrap(),
        "only one worker may claim the single queued job"
    );
    let result: Value = http
        .get(format!(
            "{base}/api/v2/osint/runs/{}",
            queued["id"].as_str().unwrap()
        ))
        .header(header::COOKIE, cookie)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(result["report"]["materials"].as_array().unwrap().len(), 2);
    assert_eq!(result["report"]["dependency_groups"], 1);
    topic_search(state, http, base, cookie).await;
}

async fn topic_search(state: &AppState, http: &reqwest::Client, base: &str, cookie: &str) {
    let pool = &state.pool;
    // Own feed: earlier checks mute the shared fixture feeds.
    let feed: Uuid = sqlx::query_scalar(
        "INSERT INTO feeds (title, feed_url, site_url) VALUES ('topic', $1, $1) RETURNING id",
    )
    .bind(format!("https://topic.example/{}", Uuid::new_v4()))
    .fetch_one(pool)
    .await
    .unwrap();
    let word = format!("topicprobe{}", &Uuid::new_v4().simple().to_string()[..8]);
    let article: Uuid = sqlx::query_scalar("INSERT INTO articles (feed_id, guid, url, url_hash, title, content, canonical_url, content_hash)
        VALUES ($1, $2, 'https://alpha.example/topic', $2, $3, 'Harbor closed after the storm.', 'https://alpha.example/topic', $2) RETURNING id")
        .bind(feed).bind(Uuid::new_v4().to_string()).bind(format!("{word} harbor"))
        .fetch_one(pool).await.unwrap();
    state
        .search
        .index_articles(&[crate::models::SearchDocument {
            id: article,
            feed_title: "alpha".into(),
            title: format!("{word} harbor"),
            content: "Harbor closed after the storm.".into(),
            url: "https://alpha.example/topic".into(),
            published_at: None,
        }])
        .unwrap();
    let topics = format!("{base}/api/v2/osint/topics");
    assert_eq!(
        http.post(&topics)
            .json(&json!({"query": word}))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        http.post(&topics)
            .header(header::COOKIE, cookie)
            .json(&json!({"query": "  ", "mode": "local"}))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let post = || async {
        http.post(&topics)
            .header(header::COOKIE, cookie)
            .json(&json!({"query": format!("  {word}  "), "mode": "local"}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
    };
    let queued = post().await;
    let again = post().await;
    assert_eq!(
        again["case_id"], queued["case_id"],
        "same query reuses the topic case"
    );
    assert_eq!(again["existing"], true);
    let case: Value = http
        .get(format!(
            "{base}/api/v2/osint/cases/{}",
            queued["case_id"].as_str().unwrap()
        ))
        .header(header::COOKIE, cookie)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(case["case"]["origin"], "topic");
    assert_eq!(case["case"]["title"], word);
    assert_eq!(case["case"]["event_available"], false);
    assert!(jobs::run_once(state).await.unwrap());
    let run: Value = http
        .get(format!(
            "{base}/api/v2/osint/runs/{}",
            queued["id"].as_str().unwrap()
        ))
        .header(header::COOKIE, cookie)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let materials = run["report"]["materials"].as_array().unwrap();
    assert_eq!(
        materials.len(),
        1,
        "local search hits become topic materials"
    );
    assert_eq!(materials[0]["id"], article.to_string());
    assert!(run["report"]["leads"].as_array().unwrap().is_empty());
    sqlx::query("DELETE FROM articles WHERE id = $1")
        .bind(article)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM feeds WHERE id = $1")
        .bind(feed)
        .execute(pool)
        .await
        .unwrap();
}
