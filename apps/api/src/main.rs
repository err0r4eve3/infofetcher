mod auth;
mod config;
mod db;
mod error;
mod event_agent;
mod extractor;
mod feeds;
mod hub;
mod intelligence;
mod keywords;
mod llm;
mod models;
mod net;
mod opml;
mod osint;
mod push;
mod retention;
mod routes;
mod search;
mod security;
mod state;
mod translation;
mod websub;
mod worker;
mod world;

use std::sync::Arc;

use anyhow::Context;
use tokio::{net::TcpListener, sync::mpsc};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::{config::Config, search::SearchIndex, state::AppState, worker::WorkerCommand};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "infofetcher_api=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let command = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "serve".to_string());
    let config = Config::from_env()?;
    let state = build_state(config.clone(), command == "serve").await?;

    match command.as_str() {
        "serve" => serve(state).await,
        "migrate" => Ok(()),
        "fetch-once" => {
            worker::fetch_all_once(&state).await?;
            Ok(())
        }
        "reindex" => {
            worker::reindex_all(&state).await?;
            Ok(())
        }
        "rebuild-intelligence" => {
            let count = intelligence::rebuild_all(&state).await?;
            tracing::info!(count, "story intelligence rebuilt");
            Ok(())
        }
        "recluster-recent" => {
            let hours = std::env::var("RECLUSTER_HOURS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(36);
            let min_feed_quality = std::env::var("RECLUSTER_MIN_FEED_QUALITY")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(60);
            let count = intelligence::recluster_recent(&state, hours, min_feed_quality).await?;
            tracing::info!(
                count,
                hours,
                min_feed_quality,
                "recent story intelligence rebuilt"
            );
            Ok(())
        }
        "recluster-article" => {
            let article_id = std::env::var("RECLUSTER_ARTICLE_ID")
                .context("RECLUSTER_ARTICLE_ID is required")?
                .parse::<uuid::Uuid>()
                .context("RECLUSTER_ARTICLE_ID must be a UUID")?;
            let story_id = intelligence::recluster_article(&state, article_id).await?;
            tracing::info!(%article_id, %story_id, "article story intelligence rebuilt");
            Ok(())
        }
        "run-event-agent-once" => {
            let processed = event_agent::run_once(&state).await?;
            tracing::info!(processed, "event wire agent turn completed");
            Ok(())
        }
        "prune-retention" => {
            let stats = retention::run_once(&state).await?;
            tracing::info!(
                articles = stats.articles,
                stories = stats.stories,
                worker_jobs = stats.worker_jobs,
                event_agent_runs = stats.event_agent_runs,
                "content retention completed"
            );
            Ok(())
        }
        other => anyhow::bail!("unknown command: {other}"),
    }
}

async fn build_state(config: Arc<Config>, start_worker: bool) -> anyhow::Result<AppState> {
    let pool = db::connect(&config.database_url).await?;
    db::migrate(&pool).await?;
    let search = Arc::new(SearchIndex::open(&config.index_dir)?);
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let (worker_tx, worker_rx) = mpsc::channel::<WorkerCommand>(512);
    let opencli_max_concurrency = config.opencli_max_concurrency;
    let llm_max_concurrency = config.llm_max_concurrency;
    let translation_max_concurrency = config.translation_max_concurrency;
    let state = AppState {
        config,
        pool,
        http,
        search,
        worker_tx,
        login_limiter: Arc::new(security::LoginRateLimiter::default()),
        opencli_semaphore: Arc::new(tokio::sync::Semaphore::new(opencli_max_concurrency)),
        llm_semaphore: Arc::new(tokio::sync::Semaphore::new(llm_max_concurrency)),
        translation_semaphore: Arc::new(tokio::sync::Semaphore::new(translation_max_concurrency)),
    };
    if start_worker {
        worker::spawn_worker(state.clone(), worker_rx);
        event_agent::spawn(state.clone());
        retention::spawn(state.clone());
    }
    Ok(state)
}

async fn serve(state: AppState) -> anyhow::Result<()> {
    let bind = state.config.app_bind;
    let app = routes::router(state);
    let listener = TcpListener::bind(bind).await?;
    tracing::info!(%bind, "infofetcher api listening");
    axum::serve(listener, app).await?;
    Ok(())
}
