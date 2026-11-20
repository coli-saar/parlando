# syntax=docker/dockerfile:1
FROM rust:1-bookworm AS server
WORKDIR /app
COPY rust-server/ ./rust-server/
COPY games/counterweight-hotel/server/ ./games/counterweight-hotel/server/
# The porter and the rendered walkway use the same route geometry.
COPY games/counterweight-hotel/client/web/src/scene/bridge-route.json ./games/counterweight-hotel/client/web/src/scene/
RUN cargo build --release --locked \
    --config 'patch.crates-io.parlando.path="/app/rust-server"' \
    --manifest-path games/counterweight-hotel/server/Cargo.toml

FROM node:22-bookworm-slim AS client
WORKDIR /app/js-client
COPY js-client/ ./
RUN npm ci && npm run build
WORKDIR /app/games/counterweight-hotel/client/web
COPY games/counterweight-hotel/client/web/ ./
# Match run.sh --local and resolve the SDK from this checkout.
RUN npm install --no-save --package-lock=false --install-links --no-audit --no-fund /app/js-client \
    && npm run build

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* && useradd --uid 10001 --create-home parlando
WORKDIR /app
COPY --from=server /app/games/counterweight-hotel/server/target/release/parlando-counterweight-hotel /usr/local/bin/
COPY --from=client /app/games/counterweight-hotel/client/web/dist ./client-dist
ENV PORT=8000 PARLANDO_CLIENT_DIST=/app/client-dist PARLANDO_DATABASE_URL=sqlite:////home/parlando/parlando.sqlite RUST_LOG=info
EXPOSE 8000
USER parlando
CMD ["parlando-counterweight-hotel", "--host", "0.0.0.0"]
