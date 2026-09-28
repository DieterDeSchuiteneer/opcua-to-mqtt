# syntax=docker/dockerfile:1

# rust:alpine's default target is x86_64-unknown-linux-musl, giving a static
# binary. Recent Rust is required (the AWS SDK's MSRV is very new), so track
# the latest 1.x image rather than pinning an old minor.

# --- test: unit tests + end-to-end suite -------------------------------------
# Independent of the (slow) release build, with cargo/target cache mounts so
# reruns after a code change are incremental. tests/e2e.rs spins up its own
# mock OPC UA server and MQTT broker on loopback, so no external services are
# needed:
#   docker build --target test --progress=plain .
# RUN_IGNORED=1 instead runs only the older `#[ignore]`d tests in
# tests/bridge_it.rs, which need a real Mosquitto:
#   docker compose -f docker-compose.test.yml up -d
#   docker build --target test --build-arg RUN_IGNORED=1 --network=host .
FROM rust:1-alpine AS test
RUN apk add --no-cache musl-dev pkgconfig openssl-dev openssl-libs-static protobuf-dev
WORKDIR /src
ARG FEATURES="aws-secrets,influx,dashboard"
ARG RUN_IGNORED=""
COPY Cargo.toml Cargo.lock* ./
COPY src ./src
COPY tests ./tests
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    if [ -n "$RUN_IGNORED" ]; then \
      cargo test --features "${FEATURES}" -- --ignored; \
    else \
      cargo test --features "${FEATURES}"; \
    fi

# --- build: release binary ---------------------------------------------------
FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev pkgconfig openssl-dev openssl-libs-static protobuf-dev
WORKDIR /src

ARG FEATURES="aws-secrets,influx,dashboard"

# Cache dependency builds separately from source changes.
COPY Cargo.toml Cargo.lock* ./
RUN mkdir src \
    && echo "fn main() {}" > src/main.rs \
    && echo "" > src/lib.rs \
    && cargo build --release --features "${FEATURES}" \
    && rm -rf src target/release/deps/opcua_to_mqtt-* target/release/deps/libopcua_to_mqtt-*

COPY src ./src
RUN cargo build --release --features "${FEATURES}"

# Default runtime: small base with libc, not `scratch` — the AWS SDK's
# crypto stack can need libc even in an otherwise-static build, and this
# stays a few MB. Matches the default FEATURES above (aws-secrets, influx,
# dashboard all compiled in).
FROM gcr.io/distroless/cc-debian12 AS runtime
COPY --from=build /src/target/release/opcua-to-mqtt /opcua-to-mqtt
ENTRYPOINT ["/opcua-to-mqtt"]
CMD ["/config/config.yaml"]

# Smallest possible image: pure rustls + musl, no libc needed at all — only
# valid when built with FEATURES="" (no aws-secrets/influx/dashboard), e.g.:
#   docker build --target runtime-scratch --build-arg FEATURES="" -t opcua-to-mqtt:minimal .
FROM scratch AS runtime-scratch
COPY --from=build /src/target/release/opcua-to-mqtt /opcua-to-mqtt
ENTRYPOINT ["/opcua-to-mqtt"]
CMD ["/config/config.yaml"]
