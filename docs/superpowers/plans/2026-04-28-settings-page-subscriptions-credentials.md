# Settings Page, Subscription Management, and Credentials

## First-principles decomposition

1. Real goal: move setup/management tasks out of the reading homepage, add a settings page for subscriptions, recommended feeds, OPML, and account credentials, and keep the homepage focused on user-selected folders and feeds.
2. Invariants: login/session flow, feed CRUD/fetch/import/export, article filtering/reading/searching, theme persistence, resizable reader panes, mobile layout, and existing API compatibility must keep working.
3. Constraints: the app is a Rust API plus Vite React frontend; credentials currently come from environment config only; avoid new dependencies; reuse the current feed/folder/OPML endpoints where possible.
4. Minimum viable change surface: add one credentials override table and API endpoint, move existing frontend management controls into a settings view, add feed delete/refetch controls there, and remove recommended/setup blocks from the reader sidebar.
5. Evidence of completion: backend tests pass, frontend typecheck/build pass, settings workflow is browser-checked, and the reader homepage no longer renders recommended subscriptions or setup forms.

## Milestones

- [x] Backend credentials override
  - Add migration for persisted admin credentials.
  - Authenticate against persisted credentials when present, falling back to env config.
  - Add authenticated endpoint to update username/password after confirming current password.
- [x] Settings UI
  - Add settings navigation from the authenticated reader shell.
  - Move manual feed add, folder create, recommended subscriptions, OPML import/export, logout, and account credentials into settings.
  - Add current subscription list with refetch and delete actions.
- [x] Homepage cleanup
  - Leave only user subscriptions/folders plus reader navigation in the main sidebar.
  - Preserve article filters, reading, keyboard shortcuts, and theme toggle.
- [x] Validation and QA
  - Run backend tests.
  - Run web typecheck/build.
  - Start the app and verify settings navigation and responsive rendering in a browser.

## Acceptance Criteria

- The main reader sidebar does not show recommended subscriptions, add-feed form, add-folder form, or OPML controls.
- The settings page can add a manual feed, create a folder, add recommended feeds, import/export OPML, refetch/delete feeds, and update account username/password.
- Account updates require the current password and persist in Postgres; existing env credentials remain valid until a DB override is saved.
- Existing sessions continue to work after updating the account username.
- Desktop and mobile layouts do not overlap or hide primary controls.

## Validation Commands

- `cargo test`
- `npm run typecheck:web`
- `npm run build:web`
- Browser QA against the local app after starting the dev server or built API stack.

## Risks

- Credential overrides rely on Postgres `pgcrypto` via the existing migration baseline. A database that skipped migrations will need migrations applied before settings-based password changes work.
- Full authenticated browser QA requires a running API and Postgres. Docker was unavailable in this environment, so browser QA used a mock API for UI interaction coverage.

## Current Status

- 2026-04-28: Implemented. `cargo test`, `npm run typecheck:web`, and `npm run build:web` pass. Browser QA passed against Vite with a mock API using headless Chrome DevTools Protocol; Docker/Postgres was unavailable locally.
