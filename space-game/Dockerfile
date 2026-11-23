# syntax=docker/dockerfile:1
FROM rust:1-bookworm AS builder
WORKDIR /app
ENV CARGO_TARGET_DIR=/app/target
RUN rustup target add wasm32-unknown-unknown \
    && cargo install --locked wasm-tools --version 1.253.0
COPY parlando-game-sdk/ parlando-game-sdk/
COPY rust-server/ rust-server/
COPY games/support/ games/support/
COPY space-game/server/ space-game/server/
RUN cargo build --release --locked --manifest-path space-game/server/Cargo.toml \
    --lib --target wasm32-unknown-unknown \
    && wasm-tools component new /app/target/wasm32-unknown-unknown/release/parlando_space_game.wasm -o /app/game.wasm
RUN cargo build --release --locked --manifest-path space-game/server/Cargo.toml \
    --features bundled-agents --bin space-game-with-agents

FROM node:22-bookworm-slim AS client
WORKDIR /app
COPY js-client/ js-client/
COPY space-game/client/ space-game/client/
WORKDIR /app/space-game/client
RUN npm ci --no-audit --no-fund && npm run build

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 10001 --create-home parlando
WORKDIR /app
COPY --from=builder /app/target/release/space-game-with-agents /usr/local/bin/
COPY --from=builder /app/game.wasm ./game.wasm
COPY --from=client /app/space-game/client/dist ./client-dist
ENV PORT=8000 PARLANDO_CLIENT_DIST=/app/client-dist PARLANDO_DATABASE_URL=sqlite:////data/space-game-component.sqlite RUST_LOG=info
EXPOSE 8000
USER parlando
CMD ["space-game-with-agents", "--host", "0.0.0.0", "--component", "/app/game.wasm"]
