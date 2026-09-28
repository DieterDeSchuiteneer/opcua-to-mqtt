# opcua-to-mqtt

A single-purpose OPC UA <-> MQTT bridge, written in Rust. It does one thing:
move tagged values between an OPC UA server and an MQTT broker, in either
direction, as efficiently as a small container can.

## Features

- **Bidirectional**: each mapping is `read` (OPC UA -> MQTT), `write` (MQTT
  -> OPC UA), or `both`.
- **Encryption on both sides**: OPC UA security policy/mode
  (`Basic256Sha256` + `SignAndEncrypt`, etc.) and MQTT over TLS (rustls,
  with optional mutual TLS).
- **Allow/block list**: glob patterns matched against the MQTT topic decide
  what actually crosses the bridge. Block always wins over allow.
- **Secret resolution**: any password/token field accepts a literal, an
  `env:VAR` reference, or (with `--features aws-secrets`) an
  `aws-secret:<id>#<field>` reference into AWS Secrets Manager.
- **OTel logging**: structured JSON logs to stdout always; OTLP log export
  when `otel.enabled`.
- **Optional InfluxDB sink** (`--features influx`): every bridged value is
  also written as a point.
- **Optional read-only dashboard** (`--features dashboard`): connection/tag
  status at `/`, `/status.json`, `/healthz`. Not a config editor, and has no
  built-in auth — keep it off a public network.

## Config

Copy `config.example.yaml` to `config.yaml` and edit it. See that file for
the full schema with comments. Minimal example:

```yaml
opcua:
  endpoint: "opc.tcp://localhost:4840"

mqtt:
  broker_host: "localhost"

mappings:
  - node_id: "ns=2;s=Channel1.Device1.Tag1"
    topic: "plant/line1/tag1"
```

## Build

```bash
# full functionality (aws-secrets + influx + dashboard)
docker build -t opcua-to-mqtt .

# smallest image: core bridge only, scratch base
docker build --target runtime-scratch --build-arg FEATURES="" -t opcua-to-mqtt:minimal .
```

## Run

```bash
docker run -v $(pwd)/config.yaml:/config/config.yaml opcua-to-mqtt
```

## Test

```bash
# unit tests + end-to-end scenarios; needs no external services
docker build --target test .
```

Or, with a local Rust toolchain, `cargo test --all-features`.

The end-to-end suite ([`tests/e2e.rs`](tests/e2e.rs)) runs the real bridge
between an in-process mock OPC UA server and an in-process mock MQTT broker
([`tests/support/`](tests/support)). Scenarios: read direction, write
direction, `both` mappings, block list, allow list, env-secret resolution,
bad-payload resilience, startup validation, and the dashboard. Not covered
there: MQTT TLS, OPC UA message security, InfluxDB, OTLP export and AWS
Secrets Manager, which each need their own mock infrastructure.

`scripts/verify.sh` runs the whole suite in Docker and writes a report to
`reports/latest.md` (see `CLAUDE.md` for the workflow). Docker Desktop needs
at least ~6 GB of memory to compile the OPC UA crates.

## Licensing

No dependency on the OPC Foundation's reference stacks (which require a
paid corporate membership for commercial use). The OPC UA client is
[`async-opcua`](https://github.com/FreeOpcUa/async-opcua) (MPL-2.0), an
independent implementation. MQTT is [`rumqttc`](https://github.com/bytebeamio/rumqtt)
(Apache-2.0).

## Status

This was scaffolded without a local Rust toolchain or network access to
crates.io available in the authoring environment, so it has **not yet been
compiled**. Before relying on it, run `docker build --target test .` and
work through any compile errors. Two files carry the most API-version risk
and are the most likely to need small fixes:

- [`src/opcua/client.rs`](src/opcua/client.rs) — the `async-opcua-client`
  API surface (subscriptions, monitored items, writes) was pieced together
  from documentation excerpts, not a compiler.
- [`src/otel.rs`](src/otel.rs) — the `opentelemetry`/`opentelemetry-otlp`
  builder APIs shift between minor versions.

Everything else (config parsing, the allow/block filter, mapping table,
dispatch/routing logic, the MQTT client) is ordinary, stable-API Rust and
has real unit test coverage in place already.
