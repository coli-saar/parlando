# syntax=docker/dockerfile:1
FROM rust:1-bookworm AS build
WORKDIR /src
COPY parlando-game-sdk/ parlando-game-sdk/
COPY rust-server/ rust-server/
RUN cargo build --manifest-path rust-server/Cargo.toml --release --locked --bin parlando

FROM debian:bookworm-slim
RUN apt-get update && apt-get install --no-install-recommends -y ca-certificates gosu \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 10001 --create-home --shell /usr/sbin/nologin parlando
COPY --from=build /src/rust-server/target/release/parlando /usr/local/bin/parlando
COPY --chmod=755 deploy/component-host/start.sh /usr/local/bin/start-parlando
ENV PORT=8000
EXPOSE 8000
ENTRYPOINT ["/usr/local/bin/start-parlando"]
