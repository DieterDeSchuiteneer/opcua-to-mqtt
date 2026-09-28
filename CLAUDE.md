# opcua-to-mqtt — instructions for Claude

Rust OPC UA <-> MQTT bridge. See README.md for features and layout. There is
no local Rust toolchain: everything is built and tested through Docker.

## Required workflow for EVERY feature request (and bug fix)

Do not report a change as done until this loop has finished green.

1. **Implement** the feature, including unit tests for new pure logic and,
   when the behavior is observable across the bridge, an end-to-end scenario
   in `tests/e2e.rs` (mocks live in `tests/support/`).
2. **Verify**: run `scripts/verify.sh`. It runs `docker build --target test`
   (unit tests + e2e suite) and writes `reports/<timestamp>-report.md`, also
   copied to `reports/latest.md`. The build is long the first time; reruns
   are incremental via cache mounts. Run it in the background and wait for
   completion rather than polling.
3. **Process the report**: read `reports/latest.md`. For every compile error,
   failing test, or new warning, find the root cause and fix it in the code
   (or fix the test if the test is what's wrong — say which and why). Never
   weaken, skip, or `#[ignore]` a test just to get green.
4. **Re-run** `scripts/verify.sh` and repeat 3–4 until the report says PASS.
   If a failure can't be fixed within reason, stop and tell the user what is
   failing and why instead of hiding it.
5. **Finish** with a short summary: what changed, the final report result
   (PASS/FAIL, test counts), and anything the suite doesn't cover.

`reports/` is gitignored; do not commit reports. Only commit when the user
asks.

## Notes

- If Docker isn't running, start Docker Desktop
  (`C:\Program Files\Docker\Docker\Docker Desktop.exe`) and wait for
  `docker info` to succeed.
- Not covered by the e2e suite (needs its own mocks): MQTT TLS, OPC UA
  message security, InfluxDB, OTLP export, AWS Secrets Manager. When a
  feature touches these, add the missing mock/test as part of the feature.
- `src/opcua/client.rs`, `src/otel.rs` and `tests/support/mock_opcua.rs`
  depend on external crate APIs that were written from docs, not a compiler;
  expect fixes there when versions move.
