# opcua-to-mqtt

Lightweight OPC UA to MQTT bridge, written in Rust. Subscribes to a configured
list of OPC UA nodes and republishes value changes to MQTT topics.

Built for a small, fast container footprint: static musl binary on a `scratch`
base image.

Uses:
- [`opcua`](https://github.com/locka99/opcua) (MIT/Apache-2.0) for the OPC UA client
- [`rumqttc`](https://github.com/bytebeamlabs/rumqtt) (Apache-2.0) for MQTT

No dependency on the OPC Foundation's reference stacks, so no GPL/membership
licensing to worry about.

## Config

Copy `config.example.yaml` to `config.yaml` and edit it:

```yaml
opcua:
  endpoint: "opc.tcp://localhost:4840"
  poll_interval_ms: 1000
  subscriptions:
    - node_id: "ns=2;s=Channel1.Device1.Tag1"
      topic: "plant/line1/tag1"

mqtt:
  broker_host: "localhost"
  broker_port: 1883
  client_id: "opcua-to-mqtt"
  qos: 0
```

## Run locally

```bash
cargo run -- config.yaml
```

## Build the container

```bash
docker build -t opcua-to-mqtt .
docker run -v $(pwd)/config.yaml:/config/config.yaml opcua-to-mqtt
```

## Status

Scaffolded without a local Rust toolchain available, so `cargo build` has not
been run yet in this environment. Before relying on it, run `cargo build` and
fix any compile errors — the `opcua` crate's exact API can shift between
versions, so `src/bridge.rs` in particular may need small adjustments to
match whatever `0.12.x` resolves to (check with `cargo doc --open` or
docs.rs/opcua).
