# syntax=docker/dockerfile:1

# rust:alpine's default target is x86_64-unknown-linux-musl, giving a static
# binary. Cache dependency builds separately from source changes.
FROM rust:1.82-alpine AS build
RUN apk add --no-cache musl-dev pkgconfig openssl-dev openssl-libs-static protobuf-dev
WORKDIR /src

ARG FEATURES="aws-secrets,influx,dashboard"

COPY Cargo.toml ./
RUN mkdir src \
    && echo "fn main() {}" > src/main.rs \
    && echo "" > src/lib.rs \
    && cargo build --release --features "${FEATURES}" \
    && rm -rf src target/release/deps/opcua_to_mqtt-* target/release/deps/libopcua_to_mqtt-*

COPY src ./src
RUN cargo build --release --features "${FEATURES}"

# Runs the unit test suite (no external services needed) at *build* time, so
# `docker build --target test .` fails the build on a failing test without
# needing a local Rust install. Integration tests need real services
# reachable from the build container's network (mosquitto via
# docker-compose, plus --network=host or a shared compose network) so they
# aren't run here by default — run them with:
#   docker compose -f docker-compose.test.yml up -d
#   docker build --target test --build-arg RUN_IGNORED=1 --network=host .
FROM build AS test
# ARGs don't cross a FROM boundary automatically — redeclare.
ARG FEATURES="aws-secrets,influx,dashboard"
COPY tests ./tests
ARG RUN_IGNORED=""
RUN if [ -n "$RUN_IGNORED" ]; then \
      cargo test --features "${FEATURES}" -- --ignored; \
    else \
      cargo test --features "${FEATURES}"; \
    fi

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
