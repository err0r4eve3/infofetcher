use std::sync::Arc;

use sqlx::PgPool;
use tokio::sync::{Semaphore, mpsc};

use crate::{
    config::Config, search::SearchIndex, security::LoginRateLimiter, worker::WorkerCommand,
};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub pool: PgPool,
    pub http: reqwest::Client,
    pub search: Arc<SearchIndex>,
    pub worker_tx: mpsc::Sender<WorkerCommand>,
    pub login_limiter: Arc<LoginRateLimiter>,
    pub opencli_semaphore: Arc<Semaphore>,
    pub llm_semaphore: Arc<Semaphore>,
    pub translation_semaphore: Arc<Semaphore>,
}
