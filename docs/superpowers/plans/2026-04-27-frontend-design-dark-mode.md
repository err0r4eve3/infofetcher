# Frontend Design Polish And Dark Mode Plan

**Goal:** Improve the RSS reader's existing three-column front-end design and add a user-controlled dark mode without changing product behavior or API contracts.

## First-Principles Decomposition

1. **Real user goal:** Make the reader feel more polished and comfortable for long reading sessions, with a dark mode that can be toggled and remembered.
2. **Invariants:** Login, feed management, article selection, search, filtering, summaries, OPML import/export, keyboard shortcuts, and API requests must keep working.
3. **Constraints:** Existing React/Vite/TypeScript app, no new dependencies, no backend changes, reuse the current single `App.tsx` and `styles.css` structure.
4. **Minimum viable change surface:** Add theme state and a brand-bar theme button in `App.tsx`; convert CSS colors to theme variables and refine the current layout/states in `styles.css`.
5. **Evidence of completion:** Typecheck/build pass; browser QA verifies login shell, mocked reader state, theme toggle persistence, desktop and mobile layouts, and no obvious overlap/contrast regressions.

## Milestones

- [x] Audit current front-end structure and styling.
- [x] Add theme state, persistence, document theme attribute, and an accessible toggle.
- [x] Refactor styles to tokenized light/dark themes and improve visual hierarchy.
- [x] Validate with deterministic commands.
- [x] Run browser/visual QA for desktop and mobile, including dark mode.

## Acceptance Criteria

- A visible theme toggle is available in the authenticated reader UI.
- Theme preference persists across reloads through local storage.
- Light mode remains consistent with the existing RSS reader concept.
- Dark mode covers login, boot, sidebar, article list, reader pane, summary panel, forms, buttons, empty states, notices, and responsive layout.
- No new dependencies or backend API changes are introduced.

## Validation Commands

```bash
npm run typecheck:web
npm run build:web
```

## Risks

- Full end-to-end authenticated QA may require a running API/Postgres. If unavailable, browser QA should use mocked API responses for UI validation and clearly report that limit.
- CSS tokenization can accidentally miss low-visibility states, so focused checks should include notices, empty state, article list selection, summary panel, and mobile stacking.

## Current Status

- Completed.
- `npm run typecheck:web` passes.
- `npm run build:web` passes.
- Browser QA used Playwright with mocked API responses because the UI can be validated without a running Postgres/API stack.
- Verified desktop light mode, desktop dark mode, mobile dark mode, login dark mode, theme persistence after reload, article selection, summary empty state, fallback extraction notice, and that mobile viewport changes do not persist collapsed desktop pane widths.
