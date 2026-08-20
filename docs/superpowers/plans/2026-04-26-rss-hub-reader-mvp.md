# RSS Hub Reader MVP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a self-hosted single-user RSS Hub reader with Rust API, React UI, Postgres persistence, Tantivy search, built-in Hub adapters, and OpenAI-compatible summaries.

**Architecture:** A single Rust service exposes API routes, serves the built React bundle, owns background fetch/summary/index jobs, and stores source-of-truth data in Postgres. The React app talks to the API with cookie credentials and renders a three-column reading surface.

**Tech Stack:** Rust `axum`, `tokio`, `sqlx`, Postgres, Tantivy, React, Vite, TypeScript, Docker Compose.

---

## Milestones

- [x] Create independent repository and feature branch.
- [x] Add database schema, backend modules, and tests.
- [x] Add React three-column UI and API client.
- [x] Add Docker, README, and operational commands.
- [x] Run cargo, npm, Compose config, and browser validation.

## Acceptance Criteria

- Admin can log in and use the reader UI.
- User can add a feed, fetch articles, read article content, mark read/favorite, search, and request a summary.
- `/hub/github-releases/:owner/:repo`, `/hub/hn-top`, and `/hub/echo` return RSS XML.
- OPML import/export works for basic feed outlines.
- Postgres remains the source of truth and Tantivy can be rebuilt from stored articles.

## Validation Commands

```bash
cargo test
cargo clippy -- -D warnings
npm run typecheck:web
npm run build:web
docker compose up --build
```

## Risks

- External feed and Hub adapter tests must avoid depending on live networks.
- LLM summaries require a configured OpenAI-compatible key; missing configuration is reported as a task failure instead of blocking article ingestion.
- Parent `/Users/error4ever/Dev/Projects` git repository is intentionally ignored; this project owns its own `.git`.

## Current Status

- Implemented on branch `feat/rss-reader-mvp`.
- `./scripts/check.sh` passes.
- `docker compose config` passes.
- `docker compose up -d postgres` could not run because the local Docker daemon was not available at `/Users/error4ever/.orbstack/run/docker.sock`.
- Browser QA used Playwright with mocked API responses because Postgres/API could not be started without Docker.
