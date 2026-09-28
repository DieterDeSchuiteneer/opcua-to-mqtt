//! End-to-end scenarios: the real bridge (`bridge::run`, driven by a real
//! parsed YAML config) between an in-process mock OPC UA server and an
//! in-process mock MQTT broker. No Docker or external services needed.
//!
//! Scenarios (each maps to a requirement in the project spec):
//!   1. read direction      — OPC UA change -> MQTT publish
//!   2. write direction     — MQTT message  -> OPC UA write
//!   3. both direction      — one mapping flowing each way
//!   4. block list          — blocked topics never cross, in either direction
//!   5. allow list          — only allow-listed topics cross
//!   6. secret resolution   — `env:` password reaches the broker in CONNECT
//!   7. bad input           — an unwritable MQTT payload doesn't kill the bridge
//!   8. startup validation  — duplicate mappings fail before any connection
//!   9. dashboard           — last-seen values exposed over HTTP (feature)
//!
//! Not covered here (need dedicated mock infrastructure): MQTT TLS, OPC UA
//! message security, InfluxDB, OTLP export, AWS Secrets Manager.

mod support;

use std::time::Duration;

use opcua_to_mqtt::bridge;
use opcua_to_mqtt::config::Config;
use opcua_types::Variant;
use support::mock_mqtt::MockMqttBroker;
use support::mock_opcua::MockOpcUaServer;
use support::{eventually, free_port};
use tokio::task::JoinHandle;

const TEMPERATURE: &str = "plant/line1/temperature";
const SETPOINT: &str = "plant/line1/setpoint/set";
const VALVE: &str = "plant/line1/valve";
const DEBUG: &str = "plant/line1/debug/counter";

fn mock_nodes() -> Vec<(&'static str, Variant)> {
    vec![
        ("Temperature", Variant::Double(20.0)),
        // Int64 so the bridge's MQTT-number -> Variant::Int64 write matches
        // the node's data type.
        ("SetPoint", Variant::Int64(0)),
        ("Valve", Variant::Boolean(false)),
        ("DebugCounter", Variant::Int32(0)),
    ]
}

/// `mqtt_extra` is extra (2-space indented) keys for the `mqtt:` section;
/// `top_level` is extra top-level sections (filters, dashboard, ...).
fn bridge_config(opcua: &MockOpcUaServer, mqtt_port: u16, mqtt_extra: &str, top_level: &str) -> Config {
    let yaml = format!(
        r#"
opcua:
  endpoint: "opc.tcp://127.0.0.1:{opcua_port}"
  poll_interval_ms: 100
mqtt:
  broker_host: "127.0.0.1"
  broker_port: {mqtt_port}
  client_id: "e2e-bridge"
{mqtt_extra}
mappings:
  - node_id: "{temperature}"
    topic: "{TEMPERATURE}"
    direction: read
  - node_id: "{setpoint}"
    topic: "{SETPOINT}"
    direction: write
  - node_id: "{valve}"
    topic: "{VALVE}"
    direction: both
  - node_id: "{debug}"
    topic: "{DEBUG}"
    direction: read
{top_level}
"#,
        opcua_port = opcua.port,
        temperature = opcua.node_id("Temperature"),
        setpoint = opcua.node_id("SetPoint"),
        valve = opcua.node_id("Valve"),
        debug = opcua.node_id("DebugCounter"),
    );
    serde_yaml::from_str(&yaml).expect("test config should parse")
}

struct Harness {
    broker: MockMqttBroker,
    opcua: MockOpcUaServer,
    bridge: JoinHandle<anyhow::Result<()>>,
}

impl Harness {
    async fn start(mqtt_extra: &str, top_level: &str) -> Self {
        let broker = MockMqttBroker::start().await;
        let opcua = MockOpcUaServer::start(mock_nodes()).await;
        let config = bridge_config(&opcua, broker.port, mqtt_extra, top_level);
        let bridge = tokio::spawn(bridge::run(config));
        Self { broker, opcua, bridge }
    }

    /// The bridge is fully up once the initial value of an allowed read
    /// node has come through (OPC UA reports the current value when a
    /// monitored item is created).
    async fn wait_until_bridging(&self) {
        self.broker.wait_for_publish(TEMPERATURE, "20.0").await;
    }

    fn writes_contain(&self, node: &str, matches: impl Fn(&Variant) -> bool) -> bool {
        self.opcua.writes_to(node).iter().any(matches)
    }

    fn assert_still_running(&self) {
        assert!(!self.bridge.is_finished(), "bridge task exited unexpectedly");
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.bridge.abort();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_direction_publishes_opcua_changes_to_mqtt() {
    let h = Harness::start("", "").await;

    // initial value, then a live change
    h.broker.wait_for_publish(TEMPERATURE, "20.0").await;
    h.opcua.set("Temperature", Variant::Double(23.5));
    h.broker.wait_for_publish(TEMPERATURE, "23.5").await;

    h.opcua.set("DebugCounter", Variant::Int32(7));
    h.broker.wait_for_publish(DEBUG, "7").await;
    h.assert_still_running();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn write_direction_forwards_mqtt_messages_to_opcua() {
    let h = Harness::start("", "").await;
    h.broker.wait_for_subscription(SETPOINT).await;

    h.broker.inject(SETPOINT, b"75");
    eventually("SetPoint write of 75", Duration::from_secs(10), || {
        h.writes_contain("SetPoint", |v| matches!(v, Variant::Int64(75)))
    })
    .await;

    h.broker.inject(SETPOINT, b"80");
    eventually("SetPoint write of 80", Duration::from_secs(10), || {
        h.writes_contain("SetPoint", |v| matches!(v, Variant::Int64(80)))
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn both_direction_mapping_flows_each_way() {
    let h = Harness::start("", "").await;
    h.wait_until_bridging().await;
    h.broker.wait_for_subscription(VALVE).await;

    // OPC UA -> MQTT
    h.opcua.set("Valve", Variant::Boolean(true));
    h.broker.wait_for_publish(VALVE, "true").await;

    // MQTT -> OPC UA
    h.broker.inject(VALVE, b"false");
    eventually("Valve write of false", Duration::from_secs(10), || {
        h.writes_contain("Valve", |v| matches!(v, Variant::Boolean(false)))
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn block_list_keeps_topics_off_the_bridge_in_both_directions() {
    let h = Harness::start(
        "",
        r#"
filters:
  block: ["plant/line1/debug/**", "plant/line1/setpoint/set"]
"#,
    )
    .await;
    h.wait_until_bridging().await;

    // blocked read topic: change it, then prove the bridge is live with a
    // control change and give a blocked publish time to (wrongly) appear.
    h.opcua.set("DebugCounter", Variant::Int32(5));
    h.opcua.set("Temperature", Variant::Double(30.0));
    h.broker.wait_for_publish(TEMPERATURE, "30.0").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        h.broker.published_on(DEBUG).is_empty(),
        "blocked read topic was published: {:?}",
        h.broker.published_on(DEBUG)
    );

    // blocked write topic: the bridge must not even subscribe to it, and a
    // message on it must never reach OPC UA.
    assert!(!h.broker.subscribed_filters().iter().any(|f| f == SETPOINT));
    h.broker.inject(SETPOINT, b"99");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(h.opcua.writes_to("SetPoint").is_empty());
    h.assert_still_running();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn allow_list_lets_only_listed_topics_through() {
    let h = Harness::start(
        "",
        r#"
filters:
  allow: ["plant/line1/temperature", "plant/line1/setpoint/*"]
"#,
    )
    .await;
    h.wait_until_bridging().await;
    h.broker.wait_for_subscription(SETPOINT).await;

    // allowed write works...
    h.broker.inject(SETPOINT, b"42");
    eventually("allowed SetPoint write", Duration::from_secs(10), || {
        h.writes_contain("SetPoint", |v| matches!(v, Variant::Int64(42)))
    })
    .await;

    // ...while unlisted read topics stay off MQTT even after changing.
    h.opcua.set("Valve", Variant::Boolean(true));
    h.opcua.set("DebugCounter", Variant::Int32(9));
    h.opcua.set("Temperature", Variant::Double(31.0));
    h.broker.wait_for_publish(TEMPERATURE, "31.0").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(h.broker.published_on(VALVE).is_empty());
    assert!(h.broker.published_on(DEBUG).is_empty());
    assert!(!h.broker.subscribed_filters().iter().any(|f| f == VALVE));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mqtt_password_is_resolved_from_an_env_secret() {
    std::env::set_var("E2E_MQTT_PASSWORD_SECRET", "s3cret");
    let h = Harness::start(
        "  username: \"bridge-user\"\n  password: \"env:E2E_MQTT_PASSWORD_SECRET\"",
        "",
    )
    .await;
    h.wait_until_bridging().await;

    assert!(
        h.broker
            .credentials()
            .contains(&(Some("bridge-user".to_string()), Some("s3cret".to_string()))),
        "broker saw credentials: {:?}",
        h.broker.credentials()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unwritable_mqtt_payload_does_not_stop_the_bridge() {
    let h = Harness::start("", "").await;
    h.broker.wait_for_subscription(SETPOINT).await;

    // a string can't be written to an Int64 node; the bridge should log
    // and carry on rather than die.
    h.broker.inject(SETPOINT, b"not a number");
    h.broker.inject(SETPOINT, b"80");
    eventually("valid write after bad one", Duration::from_secs(10), || {
        h.writes_contain("SetPoint", |v| matches!(v, Variant::Int64(80)))
    })
    .await;
    assert!(!h.writes_contain("SetPoint", |v| matches!(v, Variant::String(_))));
    h.assert_still_running();
}

#[tokio::test]
async fn duplicate_mappings_fail_startup_before_any_connection() {
    let yaml = format!(
        r#"
opcua:
  endpoint: "opc.tcp://127.0.0.1:{}"
mqtt:
  broker_host: "127.0.0.1"
  broker_port: {}
mappings:
  - node_id: "ns=2;s=A"
    topic: "plant/dup"
  - node_id: "ns=2;s=B"
    topic: "plant/dup"
"#,
        free_port(),
        free_port()
    );
    let config: Config = serde_yaml::from_str(&yaml).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(5), bridge::run(config))
        .await
        .expect("startup validation should fail fast, not try to connect");
    let err = result.expect_err("duplicate mappings must be rejected");
    assert!(
        err.to_string().contains("duplicate read mapping"),
        "unexpected error: {err:#}"
    );
}

#[cfg(feature = "dashboard")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dashboard_reports_last_seen_values() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn http_get(addr: &str, path: &str) -> Option<String> {
        let mut stream = tokio::net::TcpStream::connect(addr).await.ok()?;
        let request = format!("GET {path} HTTP/1.0\r\nHost: localhost\r\n\r\n");
        stream.write_all(request.as_bytes()).await.ok()?;
        let mut response = String::new();
        stream.read_to_string(&mut response).await.ok()?;
        Some(response)
    }

    let dashboard_addr = format!("127.0.0.1:{}", free_port());
    let h = Harness::start(
        "",
        &format!("dashboard:\n  enabled: true\n  bind: \"{dashboard_addr}\"\n"),
    )
    .await;
    h.wait_until_bridging().await;
    h.opcua.set("Temperature", Variant::Double(26.5));
    h.broker.wait_for_publish(TEMPERATURE, "26.5").await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(body) = http_get(&dashboard_addr, "/status.json").await {
            if body.contains(TEMPERATURE) && body.contains("26.5") {
                break;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "dashboard never showed the last-seen temperature"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let health = http_get(&dashboard_addr, "/healthz").await.expect("healthz");
    assert!(health.ends_with("ok"), "healthz response: {health}");
}
