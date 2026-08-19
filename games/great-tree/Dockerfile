# syntax=docker/dockerfile:1

FROM rust:1-bookworm AS builder

WORKDIR /app
COPY rust-server ./rust-server
COPY games/great-tree/server ./games/great-tree/server
RUN cargo build --release --locked --manifest-path games/great-tree/server/Cargo.toml

FROM node:22-bookworm-slim AS client-builder

WORKDIR /app
COPY js-client ./js-client
COPY games/great-tree/client ./games/great-tree/client
RUN cd js-client \
    && npm ci --ignore-scripts \
    && npm run build
RUN cd games/great-tree/client \
    && npm ci --ignore-scripts \
    && npm run build

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home --home-dir /home/parlando parlando

WORKDIR /app
COPY --from=builder /app/games/great-tree/server/target/release/parlando-great-tree /usr/local/bin/parlando-great-tree
COPY --from=client-builder /app/games/great-tree/client/dist /app/client-dist
ENV RUST_LOG=info
ENV PORT=8000
ENV PARLANDO_DATABASE_URL=sqlite:////data/parlando.sqlite
ENV PARLANDO_CLIENT_DIST=/app/client-dist
EXPOSE 8000
USER 10001:10001

CMD ["parlando-great-tree", "--host", "0.0.0.0"]
