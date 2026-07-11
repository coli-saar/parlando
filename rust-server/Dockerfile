# syntax=docker/dockerfile:1

FROM rust:1-bookworm AS builder

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release -p parlando-space-game

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/parlando-space-game /usr/local/bin/parlando-space-game
COPY config/experiment.render.example.yaml /app/config/experiment.yaml

ENV RUST_LOG=info
EXPOSE 8000

CMD ["parlando-space-game", "--host", "0.0.0.0", "--config", "/app/config/experiment.yaml"]
