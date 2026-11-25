FROM rust:bookworm AS rust-toolchain

FROM python:3.12-slim-bookworm AS build
COPY --from=rust-toolchain /usr/local/cargo /usr/local/cargo
COPY --from=rust-toolchain /usr/local/rustup /usr/local/rustup
ENV PATH="/usr/local/cargo/bin:${PATH}" CARGO_HOME=/usr/local/cargo RUSTUP_HOME=/usr/local/rustup
RUN apt-get update && apt-get install -y --no-install-recommends build-essential pkg-config libssl-dev protobuf-compiler && rm -rf /var/lib/apt/lists/*
COPY . /build/parlando-rl/
RUN python -m pip wheel --no-deps --wheel-dir /wheels /build/parlando-rl/vendor/parlando-agent-sdk /build/parlando-rl

FROM python:3.12-slim-bookworm
WORKDIR /app
COPY --from=build /wheels /wheels
RUN set -- /wheels/parlando_rl-*.whl && python -m pip install --no-cache-dir "${1}[openenv]" /wheels/parlando_remote_agent-*.whl && rm -rf /wheels
COPY deployment/ /app/deployment/
ENV PARLANDO_OPENENV_CONFIG=/app/deployment/environment.yaml
ENV ENABLE_WEB_INTERFACE=false
EXPOSE 8000
CMD ["server"]
