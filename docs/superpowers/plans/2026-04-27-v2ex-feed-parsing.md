# V2EX Feed Parsing Fix Plan

## Goal

Fix V2EX article parsing so the reader stores readable Feed content instead of V2EX page chrome, and expose practical ready-made V2EX sources in the UI.

## Milestones

- [x] Confirm the root cause from deployed data and live V2EX feeds.
- [x] Add tests for Feed HTML cleanup and V2EX feed-first extraction.
- [x] Implement the smallest parser/extractor changes.
- [x] Add useful V2EX official feeds to recommended subscriptions.
- [x] Validate locally, deploy to homeserver, and refresh the V2EX feed.

## Acceptance Criteria

- V2EX Feed HTML is stored as readable text, not literal `<p>`, `<br>`, or page template text.
- V2EX article URLs do not trigger original-page extraction when Feed content is available.
- Short V2EX posts no longer store navigation/sidebar text such as `Home Sign Up Sign In`.
- Recommended subscriptions include working official V2EX sources beyond the all-site index.
- The public deployment at `https://rss-reader.error-forever.com` serves the fixed build.

## Validation Commands

```bash
cargo test -p rss-hub-reader-api
npm run typecheck:web
npm run build:web
curl -fsS https://rss-reader.error-forever.com/api/health
```

## Risks

- V2EX entries with empty Feed content will remain empty instead of falling back to an unreliable page scrape.
- Public RSSHub instances may be Cloudflare-blocked from this runtime, so recommended defaults should prefer official V2EX feeds.

## Current Status

Completed and deployed to the homeserver-backed public domain. Root cause confirmed: V2EX official Atom feeds contain post HTML, but the reader treated short Feed content as insufficient and fetched the original V2EX page. The generic extractor then selected `body`, which included navigation and sidebar text. The fix now cleans Feed HTML before storage, treats V2EX article URLs as Feed-first, strips URL fragments from article hashes so `#replyN` changes do not break upserts, and updates existing articles by `(feed_id, guid)` before falling back to URL-hash upsert. The existing V2EX feed was refreshed and the remaining historical V2EX page-chrome rows were cleaned from the deployed database.
