# Feed Categories, Media Presets, and OPML

## Goal

Add Hostloc and established media subscriptions, make subscription categories usable in the reader, and preserve categories through OPML import/export with a Folo-like source organization pattern.

## Milestones

- [x] Verify candidate feed URLs and avoid broken or unofficial feeds where practical.
- [x] Extend backend OPML parsing/rendering to round-trip folder/category structure.
- [x] Extend frontend presets with categories and add category-aware subscription flows.
- [x] Improve sidebar source organization so folders can be selected as article filters.
- [x] Validate with unit tests, frontend typecheck/build, and browser QA.
- [x] Deploy the verified build to the existing public homeserver endpoint.

## Acceptance Criteria

- Hostloc is available as a ready-made feed.
- Several established media feeds are available as ready-made feeds under a media category.
- Adding a preset creates or reuses its folder and stores the feed in that folder.
- Manual feed add can target an existing folder.
- Folder rows can be selected to filter the article list.
- OPML export emits nested folder outlines; import recreates folders and assigns feeds.
- Existing flat OPML import/export continues to work.

## Validation Commands

- `cargo test`
- `npm run typecheck:web`
- `npm run build:web`
- Browser login and flow check against the deployed public URL.

## Risks

- Some publishers may rate-limit or block automated fetches even when their feed URL responds in curl.
- OPML folder conventions vary between readers; implement standard nested outline support and keep flat outlines compatible.
- Reuters/AP do not appear to provide stable official public general-news RSS feeds, so they should not be included as default presets unless a reliable source is confirmed.

## Current Status

Completed and deployed to `https://rss-reader.error-forever.com`. `cargo test`, `npm run typecheck:web`, and `npm run build:web` pass. Public API health, deployed asset names, OPML category export, feed ingestion, and browser QA on desktop and mobile-width layouts were verified.
