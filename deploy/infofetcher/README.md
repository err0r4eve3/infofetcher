# Infofetcher public entry

The public URL is `https://infofetcher.error.moe`. Wrangler deploys a small Worker
with a custom domain; Cloudflare manages its DNS and edge certificate. The Worker
forwards requests to the existing `https://error.moe` origin, preserving
paths, request bodies and session cookies. PostgreSQL, ingestion and LLM jobs
continue running on the existing server.

Deploy from the repository root using an authorized Wrangler account:

```sh
npx wrangler@4 deploy --config deploy/infofetcher/wrangler.jsonc
```

Keep the origin hostname available. Set the backend's `APP_PUBLIC_URL` to
`https://infofetcher.error.moe` so generated URLs use the public entry. Do not use
`error.moe` as the deployment target. Verify login, authenticated report/detail
requests and deployed asset hashes through the custom domain after each release.
