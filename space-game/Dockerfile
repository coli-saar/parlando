# syntax=docker/dockerfile:1

FROM rust:1-bookworm AS builder

WORKDIR /app
COPY rust-server ./rust-server
COPY space-game/server ./space-game/server
RUN cargo build --release --locked --manifest-path space-game/server/Cargo.toml

FROM node:22-bookworm-slim AS client-builder

WORKDIR /app
COPY js-client ./js-client
COPY space-game/client ./space-game/client
RUN cd js-client \
    && npm ci --ignore-scripts \
    && npm run build
RUN cd space-game/client \
    && npm ci --ignore-scripts \
    && npm run build

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home --home-dir /home/parlando parlando

WORKDIR /app
COPY --from=builder /app/space-game/server/target/release/parlando-space-game /usr/local/bin/parlando-space-game
COPY --from=client-builder /app/space-game/client/dist /app/client-dist
ENV RUST_LOG=info
ENV PORT=8000
ENV PARLANDO_DATABASE_URL=sqlite:////data/parlando.sqlite
ENV PARLANDO_CLIENT_DIST=/app/client-dist
EXPOSE 8000
USER 10001:10001

CMD ["parlando-space-game", "--host", "0.0.0.0"]
