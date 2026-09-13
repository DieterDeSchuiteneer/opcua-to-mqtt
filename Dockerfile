# Builds a static musl binary (rust:alpine's default target is
# x86_64-unknown-linux-musl), so the runtime image can be `scratch`.
FROM rust:1.76-alpine AS build
RUN apk add --no-cache musl-dev pkgconfig openssl-dev openssl-libs-static
WORKDIR /src

# Cache dependency builds separately from source changes.
COPY Cargo.toml ./
RUN mkdir src && echo "fn main() {}" > src/main.rs \
    && cargo build --release \
    && rm -rf src

COPY src ./src
RUN touch src/main.rs && cargo build --release

FROM scratch
COPY --from=build /src/target/release/opcua-to-mqtt /opcua-to-mqtt
ENTRYPOINT ["/opcua-to-mqtt"]
CMD ["/config/config.yaml"]
