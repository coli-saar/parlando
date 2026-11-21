# syntax=docker/dockerfile:1
FROM rust:1-bookworm AS builder
WORKDIR /app
ENV CARGO_TARGET_DIR=/app/target
RUN rustup target add wasm32-unknown-unknown \
    && cargo install --locked wasm-tools --version 1.253.0
COPY parlando-game-sdk/ parlando-game-sdk/
COPY rust-server/ rust-server/
COPY games/support/ games/support/
COPY games/counterweight-hotel/server/ games/counterweight-hotel/server/
COPY games/counterweight-hotel/client/web/src/scene/bridge-route.json games/counterweight-hotel/client/web/src/scene/
RUN cargo build --release --locked --manifest-path games/counterweight-hotel/server/Cargo.toml \
    --lib --target wasm32-unknown-unknown \
    && wasm-tools component new /app/target/wasm32-unknown-unknown/release/parlando_counterweight_hotel.wasm -o /app/game.wasm
RUN cargo build --release --locked --manifest-path games/counterweight-hotel/server/Cargo.toml \
    --features bundled-agents --bin counterweight-hotel-with-agents

FROM node:22-bookworm-slim AS client
WORKDIR /app
COPY games/counterweight-hotel/client/web/ ./
RUN npm ci --no-audit --no-fund && npm run build

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 10001 --create-home parlando
WORKDIR /app
COPY --from=builder /app/target/release/counterweight-hotel-with-agents /usr/local/bin/
COPY --from=builder /app/game.wasm ./game.wasm
COPY --from=client /app/dist ./client-dist
ENV PORT=8000 PARLANDO_CLIENT_DIST=/app/client-dist PARLANDO_DATABASE_URL=sqlite:////home/parlando/hotel-component.sqlite RUST_LOG=info
EXPOSE 8000
USER parlando
CMD ["counterweight-hotel-with-agents", "--host", "0.0.0.0", "--component", "/app/game.wasm"]
