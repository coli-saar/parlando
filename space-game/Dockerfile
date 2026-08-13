# syntax=docker/dockerfile:1

FROM rust:1-bookworm AS builder

WORKDIR /app
COPY rust-server ./rust-server
COPY space-game/server ./space-game/server
RUN cargo build --release --manifest-path space-game/server/Cargo.toml

FROM node:22-bookworm-slim AS client-builder

WORKDIR /app
COPY js-client ./js-client
COPY space-game/client ./space-game/client
RUN cd js-client \
    && npm install \
    && npm run build \
    && npm pack --pack-destination /tmp
RUN cd space-game/client \
    && npm install --no-save /tmp/coli-saar-parlando-client-0.2.0.tgz \
    && npm run build

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/space-game/server/target/release/parlando-space-game /usr/local/bin/parlando-space-game
COPY --from=client-builder /app/space-game/client/dist /app/client-dist
COPY space-game/config/experiment.render.example.yaml /app/config/experiment.yaml

ENV RUST_LOG=info
EXPOSE 8000

CMD ["parlando-space-game", "--host", "0.0.0.0", "--config", "/app/config/experiment.yaml"]
