# Homeserver Cloudflare Tunnel Test Plan

## Goal

Deploy the current RSS Hub Reader build to the SSH `homeServer` host and verify the ready-made RSS subscription UI through the existing permanent Cloudflare Tunnel.

## Milestones

- [x] Confirm local and remote tooling.
- [x] Add a homeserver compose override that avoids occupied host ports.
- [x] Sync the repository to an isolated remote deployment directory.
- [x] Start Postgres and API containers on `homeServer`.
- [x] Expose the remote API/UI through a Cloudflare quick tunnel.
- [x] Open the public URL in the in-app browser and verify NodeSeek can be added.
- [x] Move the temporary API runtime to host networking so feed fetches can use the homeserver's outbound network path.
- [x] Move public access from the quick tunnel to the existing permanent Cloudflare Tunnel on `rss-reader.error-forever.com`.

## Acceptance Criteria

- The app is reachable through `https://rss-reader.error-forever.com` using the existing permanent Cloudflare Tunnel.
- The login screen uses a non-default test admin password.
- The sidebar shows the recommended feeds including `https://rss.nodeseek.com/`.
- Clicking NodeSeek creates/selects that feed and queues the existing backend fetch path.
- No existing homeserver service ports are reused.
- A reachable feed source can be fetched from the homeserver runtime.

## Validation Commands

```bash
docker compose -f docker-compose.yml -f docker-compose.homeserver.yml config
ssh homeServer 'docker compose -p rss-hub-reader-test ps'
curl -fsS http://localhost:18181/api/health
curl -fsS https://rss-reader.error-forever.com/api/health
```

## Risks

- The public URL exposes the test app, so the deployment must not use the default `change-me-now` password.
- First Docker build on the homeserver can take several minutes.
- Docker Hub and apt networking can be flaky on the homeserver; the homeserver Dockerfile uses cached Node 22 LTS and Python slim images, with apt IPv4/timeouts set for the temporary build layer.
- `rss.nodeseek.com` is not currently reachable from the homeserver network path; the app can add/select that feed and queues the fetch, but the source request times out before articles can be imported.
- The local `cloudflared` connector token can run the existing tunnel but cannot manage DNS/public-hostname routes; future route changes need Dashboard access or a management API token.

## Current Status

Homeserver SSH and Docker Compose are available. Default compose ports conflict with existing services, so a scoped override was added. The repository has been synced to `~/deployments/rss-hub-reader-test`. The test Postgres container is healthy and bound only to `127.0.0.1:15432`. Docker image/package-manager networking was too flaky for containerized builds, so Rust stable 1.95.0 was installed in the remote user's home directory via rustup and the frontend/API release builds completed natively. The API runtime container runs on host networking at `127.0.0.1:18181`, and the permanent Cloudflare Tunnel now exposes it at `https://rss-reader.error-forever.com`. `APP_PUBLIC_URL` has been updated to the permanent domain, authenticated metrics over the permanent domain return the expected public URL and feed/article counts, and the temporary quick tunnel container has been stopped. Hacker News fetched successfully from the homeserver runtime, proving the feed import path works through the deployed API. NodeSeek itself still times out from the homeserver network path. The existing `cloudflared-tunnel` container only exposes a connector `TUNNEL_TOKEN`; using that token for `cloudflared tunnel route dns --overwrite-dns ... rss-reader.error-forever.com` still requires an origin cert, and using the mounted origin cert returns Cloudflare authentication error 10000, so the final hostname/DNS route was completed outside the available CLI credentials.
