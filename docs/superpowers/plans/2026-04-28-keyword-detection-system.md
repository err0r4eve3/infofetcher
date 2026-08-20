# Keyword Detection System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a keyword detection system that can target selected feeds and folders, record matched articles, and batch-send matched articles to AI summaries or Telegram.

**Architecture:** Postgres remains the source of truth for keyword rules, rule targets, and match records. The Rust API exposes authenticated CRUD, scan, match listing, and batch-action endpoints; the existing worker detects keywords after feed ingestion and reuses the existing summary queue. The React settings page adds rule management, target selection, match review, and batch action controls.

**Tech Stack:** Rust `axum`, `sqlx`, Postgres migrations, existing worker queue, `reqwest` for Telegram Bot API, React/Vite/TypeScript.

---

## First-Principles Decomposition

1. Real goal: let the user define keyword rules, choose which feeds/folders they apply to, inspect hits, and trigger AI summaries or Telegram pushes in batches.
2. Invariants: existing feed fetching, article reading, search, summaries, OPML, settings, login/session, and mobile layout must keep working.
3. Constraints: no new dependencies unless necessary; LLM summary already exists and should be reused; Telegram credentials belong in environment config; rule targets should follow current feed/folder concepts.
4. Minimum viable change surface: add keyword tables and route handlers, add a small tested matching module, hook detection after article fetch, add settings UI panels, and document the API/env knobs.
5. Evidence of completion: targeted keyword tests fail before implementation and pass after, backend tests/clippy pass, web typecheck/build pass, and the UI workflow is browser-checked.

## Milestones

- [x] Keyword matching core
  - Add tests for keyword normalization, title/content matching, case sensitivity, and Telegram message formatting.
  - Implement the minimal pure matching helpers.
- [x] Backend persistence and actions
  - Add migration for keyword rules, feed/folder targets, and match records.
  - Add models and authenticated endpoints for rules, scans, match listing, and batch match actions.
  - Reuse the existing summary queue and add Telegram send support through env config.
  - Detect enabled keyword rules after fetched articles are upserted.
- [x] Settings UI
  - Add rule creation form with keyword textarea, title/body switches, auto-summary/TG toggles, and bulk feed/folder target checkboxes.
  - Add rule list with scan/delete actions.
  - Add match table with selection, rule filtering, and batch AI summary/TG push buttons.
- [x] Docs and validation
  - Update API/README docs for keyword endpoints and Telegram env vars.
  - Run `cargo test -p rss-hub-reader-api`, `cargo clippy -p rss-hub-reader-api -- -D warnings`, `npm run typecheck:web`, and `npm run build:web`.
  - Start the web app and browser-check settings keyword workflows on desktop and mobile.

## Acceptance Criteria

- A keyword rule can be created with one or more keywords and any mix of feed/folder targets; no target means all subscriptions.
- Rule matching supports title/body toggles and optional case sensitivity.
- A rule scan records matched articles without duplicating existing hits.
- Newly fetched articles are checked against enabled rules.
- Matched articles can be selected in bulk and queued for AI summary, Telegram push, or both.
- Telegram failures are visible per match instead of hiding the error.
- Existing reader/settings behavior remains intact.

## Validation Commands

- `cargo test -p rss-hub-reader-api`
- `cargo clippy -p rss-hub-reader-api -- -D warnings`
- `npm run typecheck:web`
- `npm run build:web`
- Browser QA against the local Vite app with mocked authenticated API responses if Postgres/API cannot be started locally.

## Risks

- Telegram delivery depends on `TELEGRAM_BOT_TOKEN` and `TELEGRAM_CHAT_ID`; missing or invalid credentials should mark only Telegram actions failed.
- Automatic Telegram sends on fetch can become noisy; match rows are unique by `(rule_id, article_id)` so existing hits are not resent by repeated scans/fetches.
- Full end-to-end verification needs a running Postgres/API stack; if unavailable, deterministic checks and mocked browser QA cover most of the changed behavior.

## Current Status

- 2026-04-28: Implemented. `cargo test -p rss-hub-reader-api`, `cargo clippy -p rss-hub-reader-api -- -D warnings`, `npm run typecheck:web`, and `npm run build:web` pass. Browser QA passed against a local mock API proxy on desktop and 390px mobile viewports for rule creation, target selection, scan, match selection, and AI/TG batch actions.
