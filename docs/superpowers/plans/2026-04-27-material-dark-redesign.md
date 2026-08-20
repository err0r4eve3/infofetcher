# Material Dark Mode Redesign Plan

**Goal:** Redesign the dark mode so the reader feels closer to Google News / Material 3: neutral smoky surfaces, readable gray text, restrained accents, and clear elevation without changing behavior.

## First-Principles Decomposition

1. **Real user goal:** Replace the current heavy green dark mode with a more professional, reading-friendly dark interface.
2. **Invariants:** Login, feed management, article selection, searching, filters, summaries, OPML, theme persistence, resizable panes, mobile stacking, and API behavior must not change.
3. **Constraints:** Existing React/Vite/CSS architecture, no new dependencies, no backend changes, no new design system files.
4. **Minimum viable change surface:** Refine `styles.css` dark tokens and dark-specific component states; keep the existing theme toggle logic in `App.tsx`.
5. **Evidence of completion:** Typecheck/build pass, browser QA verifies desktop/mobile dark mode, theme persistence, key reader states, and no obvious overlap or excessive contrast.

## References

- Google Material dark theme guidance: smoky grays, reduced pixel bleed, desaturated accents, elevated surfaces.
- Google News dark theme: content-first headlines on a dark gray backdrop for longform readers.
- Inoreader product pattern: practical RSS/sidebar/list/reader information architecture.

## Milestones

- [x] Rework the dark color tokens into Material-style surface roles.
- [x] Tune dark-mode buttons, feed rows, article list rows, chips, summary panels, and reader text hierarchy.
- [x] Validate with deterministic commands.
- [x] Run browser QA on desktop and mobile dark mode.

## Acceptance Criteria

- Dark mode is neutral charcoal/gray, not green-dominant.
- Primary text avoids pure white glare while remaining readable.
- Selected rows and summary panels have clear but restrained emphasis.
- The theme toggle remains visible and persists across reloads.
- Mobile dark mode remains scannable without hidden controls or text overlap.

## Validation Commands

```bash
npm run typecheck:web
npm run build:web
```

## Current Status

- Completed locally.
- `npm run typecheck:web` passes.
- `npm run build:web` passes.
- Browser QA used Playwright with mocked API responses.
- Verified desktop dark mode, mobile dark mode, login dark mode, theme persistence, article selection, empty summary state, and fallback extraction notice.
