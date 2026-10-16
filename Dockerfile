# Tameion Watch: the hosted level-0 service (crates/operator/src/bin/tameion-app.rs).
# Multi-stage: build with the pinned lockfiles, run on a slim image.
# The console (apps/console, Next.js static export) is built first; the service
# serves it at / from TAMEION_CONSOLE_DIR, the classic page stays at /classic.
FROM node:22-bookworm-slim AS console
WORKDIR /console
COPY apps/console/package.json apps/console/package-lock.json ./
RUN npm ci
COPY apps/console/ ./
RUN npm run build

FROM rust:1.95-slim-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN cargo build --locked --release -p operator --bin tameion-app

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/* \
 && useradd -m -u 10001 tameion
COPY --from=build /src/target/release/tameion-app /usr/local/bin/tameion-app
COPY --from=console /console/out /app/console
COPY --from=build /src/proof /app/proof
USER tameion
ENV HOST=0.0.0.0 PORT=8080 TAMEION_APP_DIR=/data/app TAMEION_CONSOLE_DIR=/app/console TAMEION_PROOF_DIR=/app/proof
VOLUME ["/data"]
EXPOSE 8080
CMD ["tameion-app"]
