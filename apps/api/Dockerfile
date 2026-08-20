FROM node:22-alpine AS web
WORKDIR /app
COPY package.json package-lock.json* ./
COPY apps/web/package.json apps/web/package.json
RUN npm install
COPY apps/web apps/web
RUN npm run build:web

FROM rust:1.94-slim AS api
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config ca-certificates libssl-dev && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock* ./
COPY apps/api/Cargo.toml apps/api/Cargo.toml
COPY apps/api apps/api
RUN cargo build --release -p rss-hub-reader-api

FROM debian:bookworm-slim
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=api /app/target/release/rss-hub-reader-api /usr/local/bin/rss-hub-reader-api
COPY --from=web /app/apps/web/dist /app/apps/web/dist
ENV WEB_DIST_DIR=/app/apps/web/dist
EXPOSE 8080
CMD ["rss-hub-reader-api", "serve"]
