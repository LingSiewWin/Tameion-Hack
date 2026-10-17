# Tameion: the service (crates/operator/src/bin/tameion-app.rs) and the console it serves at /.
# Multi-stage: build with the pinned lockfiles, run on a slim image. No secret enters the image:
# Fly hands them to the process as environment variables (scripts/deploy_fly.sh sets them by file).
FROM node:22-bookworm-slim AS console
WORKDIR /console
COPY apps/console/package.json apps/console/package-lock.json ./
RUN npm ci
COPY apps/console/ ./
RUN npm run build

FROM rust:1.95-slim-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --locked --release -p operator --bin tameion-app

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/* \
 && useradd -m -u 10001 tameion
# The service resolves these relative to its working directory, as it does in the repo:
# apps/console/out (the console at /), apps/classic, runtime/<chain>_anchor.addr (level1.rs anchor_for),
# proof/<name> (the two files the console links to; the rest of proof/ stays out of the image).
WORKDIR /app
COPY --from=build /src/target/release/tameion-app /usr/local/bin/tameion-app
COPY --from=console /console/out /app/apps/console/out
COPY apps/classic /app/apps/classic
COPY runtime/arc_anchor.addr runtime/arbitrum-sepolia_anchor.addr /app/runtime/
COPY proof/hedge-testnet-live-open-2026-10-13.txt proof/hedge-testnet-nowithdraw-2026-10-13.txt /app/proof/
# Everything the service writes goes to the volume: accounts, passes, bills, the book, keys it makes on first use.
ENV HOST=0.0.0.0 PORT=8080 TAMEION_APP_DIR=/data/app TAMEION_LOG_DIR=/data/log TAMEION_VAPID_KEY=/data/vapid.key \
    TAMEION_STATEMENT_KEY_FILE=/data/statement.key HEDGE_HALT_DIR=/data/hedge TAMEION_DESKS_DIR=/data/desks
VOLUME ["/data"]
EXPOSE 8080
# A Fly volume is mounted owned by root: hand it to the service's user, then drop root for good.
CMD ["sh", "-c", "chown -R tameion:tameion /data && exec setpriv --reuid=tameion --regid=tameion --init-groups tameion-app"]
