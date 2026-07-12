# syntax=docker/dockerfile:1

FROM rust:1-bookworm AS builder

WORKDIR /app
COPY rust-server ./rust-server
COPY space-game/server ./space-game/server
RUN cargo build --release --manifest-path space-game/server/Cargo.toml

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/space-game/server/target/release/parlando-space-game /usr/local/bin/parlando-space-game
COPY space-game/config/experiment.render.example.yaml /app/config/experiment.yaml

ENV RUST_LOG=info
EXPOSE 8000

CMD ["parlando-space-game", "--host", "0.0.0.0", "--config", "/app/config/experiment.yaml"]
