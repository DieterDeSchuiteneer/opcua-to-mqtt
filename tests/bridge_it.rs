//! Integration tests, `#[ignore]`d by default since they need real
//! infrastructure:
//!
//! - `mqtt_round_trip` needs `docker compose -f docker-compose.test.yml up
//!   -d` (a real Mosquitto broker on localhost:1883). It exercises the
//!   dispatcher against real MQTT in both directions, without touching the
//!   OPC UA client crate at all.
//! - `opcua_change_reaches_dispatcher` needs no external services: it spins
//!   up an in-process OPC UA test server (async-opcua-server, a
//!   dev-dependency) and drives the real OPC UA client against it. Kept
//!   separate from the MQTT test so the two highest-risk integration
//!   points (the OPC UA client crate and the MQTT broker) can fail
//!   independently rather than masking each other.
//!
//! Run with: `cargo test --test bridge_it -- --ignored`

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use opcua_to_mqtt::bridge::dispatch::{Dispatcher, MqttMessage, OpcUaChange, OpcUaWriter};
use opcua_to_mqtt::bridge::mapping;
use opcua_to_mqtt::config::{Direction, FilterConfig, Mapping};
use opcua_to_mqtt::mqtt::drive_event_loop;
use opcua_to_mqtt::sinks::{MqttSink, TelemetrySink, Value};

use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use tokio::sync::mpsc;

/// Polls `condition` until it's true or `timeout` elapses. `condition` does
/// a quick synchronous mutex check (never held across an `.await`), so a
/// plain `std::sync::Mutex` is fine here.
async fn wait_until(condition: impl Fn() -> bool, timeout: Duration) -> bool {
    let start = tokio::time::Instant::now();
    while start.elapsed() < timeout {
        if condition() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    condition()
}

#[derive(Default)]
struct RecordingSink {
    published: Mutex<Vec<(String, Value)>>,
}

#[async_trait]
impl TelemetrySink for RecordingSink {
    async fn publish(&self, topic: &str, value: &Value) -> anyhow::Result<()> {
        self.published
            .lock()
            .unwrap()
            .push((topic.to_string(), value.clone()));
        Ok(())
    }
}

#[derive(Default)]
struct RecordingWriter {
    writes: Mutex<Vec<(String, Value)>>,
}

#[async_trait]
impl OpcUaWriter for RecordingWriter {
    async fn write(&self, node_id: &str, value: &Value) -> anyhow::Result<()> {
        self.writes
            .lock()
            .unwrap()
            .push((node_id.to_string(), value.clone()));
        Ok(())
    }
}

#[tokio::test]
#[ignore = "needs `docker compose -f docker-compose.test.yml up -d`"]
async fn mqtt_round_trip() {
    let table = mapping::build(
        &[
            Mapping {
                node_id: "ns=2;s=A".into(),
                topic: "it/read".into(),
                direction: Direction::Read,
            },
            Mapping {
                node_id: "ns=2;s=B".into(),
                topic: "it/write".into(),
                direction: Direction::Write,
            },
        ],
        &FilterConfig::default(),
    )
    .unwrap();

    let mut opts = MqttOptions::new("bridge-it-bridge", "localhost", 1883);
    opts.set_keep_alive(Duration::from_secs(5));
    let (client, event_loop) = AsyncClient::new(opts, 32);

    let (mqtt_msg_tx, mut mqtt_msg_rx) = mpsc::channel::<MqttMessage>(16);
    tokio::spawn(drive_event_loop(event_loop, mqtt_msg_tx));
    client
        .subscribe("it/write", QoS::AtLeastOnce)
        .await
        .unwrap();

    let sinks: Vec<Arc<dyn TelemetrySink>> =
        vec![Arc::new(MqttSink::new(client.clone(), QoS::AtMostOnce))];
    let dispatcher = Arc::new(Dispatcher::new(&table, sinks));
    let writer = Arc::new(RecordingWriter::default());

    {
        let dispatcher = dispatcher.clone();
        let writer = writer.clone();
        tokio::spawn(async move {
            while let Some(msg) = mqtt_msg_rx.recv().await {
                dispatcher.handle_mqtt_message(msg, writer.as_ref()).await;
            }
        });
    }

    // A second, independent client plays the role of "an external MQTT
    // actor": it observes what the bridge publishes, and sends the message
    // that should trigger an OPC UA write.
    let mut observer_opts = MqttOptions::new("bridge-it-observer", "localhost", 1883);
    observer_opts.set_keep_alive(Duration::from_secs(5));
    let (observer, mut observer_loop) = AsyncClient::new(observer_opts, 32);
    observer
        .subscribe("it/read", QoS::AtLeastOnce)
        .await
        .unwrap();

    let received: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(vec![]));
    {
        let received = received.clone();
        tokio::spawn(async move {
            loop {
                match observer_loop.poll().await {
                    Ok(Event::Incoming(Packet::Publish(p))) => {
                        received.lock().unwrap().push(p.payload.to_vec());
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        });
    }

    // give subscriptions a moment to land with the broker
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Read direction: a change "from OPC UA" should reach the MQTT observer.
    dispatcher
        .handle_opcua_change(OpcUaChange {
            node_id: "ns=2;s=A".into(),
            value: Value::Int(42),
        })
        .await;

    assert!(
        wait_until(|| !received.lock().unwrap().is_empty(), Duration::from_secs(5)).await,
        "expected the observer to receive a publish on it/read"
    );

    // Write direction: an external publish on it/write should reach OPC UA
    // (represented here by the RecordingWriter).
    observer
        .publish("it/write", QoS::AtLeastOnce, false, "99")
        .await
        .unwrap();

    assert!(
        wait_until(
            || !writer.writes.lock().unwrap().is_empty(),
            Duration::from_secs(5)
        )
        .await,
        "expected the write-direction message to reach the OPC UA writer"
    );
    let writes = writer.writes.lock().unwrap();
    assert_eq!(writes[0], ("ns=2;s=B".to_string(), Value::Int(99)));
}

#[tokio::test]
#[ignore = "spins up an in-process OPC UA test server; run explicitly once built"]
async fn opcua_change_reaches_dispatcher() {
    use opcua_server::prelude::*;
    use opcua_to_mqtt::config::OpcUaConfig;
    use opcua_to_mqtt::opcua::ConnectedClient;

    let (server, handle) = ServerBuilder::new()
        .application_name("opcua-to-mqtt-test-server")
        .application_uri("urn:opcua-to-mqtt-test-server")
        .host("127.0.0.1")
        .port(4855)
        .with_node_manager(simple_node_manager(
            NamespaceMetadata {
                namespace_uri: "urn:opcua-to-mqtt-test".to_string(),
                ..Default::default()
            },
            "simple",
        ))
        .build()
        .expect("failed to build test OPC UA server");

    let node_manager = handle
        .node_managers()
        .get_of_type::<SimpleNodeManager>()
        .expect("simple node manager registered");
    let ns = handle
        .get_namespace_index("urn:opcua-to-mqtt-test")
        .expect("namespace registered");

    let node_id = NodeId::new(ns, "Tag1");
    {
        let address_space = node_manager.address_space();
        let mut address_space = address_space.write();
        VariableBuilder::new(&node_id, "Tag1", "Tag1")
            .value(0i32)
            .insert(&mut address_space);
    }

    tokio::spawn(server.run());
    tokio::time::sleep(Duration::from_millis(300)).await;

    let opcua_cfg = OpcUaConfig {
        endpoint: "opc.tcp://127.0.0.1:4855".to_string(),
        security_policy: "None".to_string(),
        security_mode: "None".to_string(),
        identity: None,
        poll_interval_ms: 200,
    };
    let client = ConnectedClient::connect(&opcua_cfg, None, None)
        .await
        .expect("failed to connect to in-process test server");

    let config_node_id = format!("ns={ns};s=Tag1");
    let table = mapping::build(
        &[Mapping {
            node_id: config_node_id.clone(),
            topic: "it/tag1".into(),
            direction: Direction::Read,
        }],
        &FilterConfig::default(),
    )
    .unwrap();

    let sink = Arc::new(RecordingSink::default());
    let sinks: Vec<Arc<dyn TelemetrySink>> = vec![sink.clone()];
    let dispatcher = Arc::new(Dispatcher::new(&table, sinks));

    let (opcua_tx, mut opcua_rx) = mpsc::channel::<OpcUaChange>(16);
    client
        .subscribe(&[config_node_id], opcua_tx)
        .await
        .expect("failed to subscribe");

    {
        let dispatcher = dispatcher.clone();
        tokio::spawn(async move {
            while let Some(change) = opcua_rx.recv().await {
                dispatcher.handle_opcua_change(change).await;
            }
        });
    }

    node_manager
        .set_value(
            &handle.subscriptions(),
            &node_id,
            None,
            DataValue::new_now(Variant::Int32(123)),
        )
        .expect("failed to set value on test server");

    assert!(
        wait_until(
            || !sink.published.lock().unwrap().is_empty(),
            Duration::from_secs(5)
        )
        .await,
        "expected the OPC UA value change to reach the dispatcher's sink"
    );
}
