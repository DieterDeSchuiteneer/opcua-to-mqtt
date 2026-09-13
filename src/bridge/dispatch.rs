use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::sinks::{TelemetrySink, Value};

use super::mapping::MappingTable;
use super::status::StatusStore;

/// A value change observed on the OPC UA side, ready to be routed to sinks.
#[derive(Debug, Clone, PartialEq)]
pub struct OpcUaChange {
    pub node_id: String,
    pub value: Value,
}

/// A message received on an MQTT write-direction topic, ready to be routed
/// to the OPC UA side.
#[derive(Debug, Clone, PartialEq)]
pub struct MqttMessage {
    pub topic: String,
    pub value: Value,
}

/// Implemented by whatever can perform an OPC UA write (the real OPC UA
/// client in production, a recording fake in tests).
#[async_trait]
pub trait OpcUaWriter: Send + Sync {
    async fn write(&self, node_id: &str, value: &Value) -> anyhow::Result<()>;
}

/// Routes OPC UA changes to MQTT (+ other sinks) and MQTT write-topic
/// messages back to OPC UA, using the mapping table built from config +
/// the allow/block list. Contains no network I/O itself, so it's fully
/// unit-testable against fakes.
pub struct Dispatcher {
    topic_by_node: HashMap<String, String>,
    node_by_topic: HashMap<String, String>,
    sinks: Vec<Arc<dyn TelemetrySink>>,
    status: Arc<StatusStore>,
}

impl Dispatcher {
    pub fn new(table: &MappingTable, sinks: Vec<Arc<dyn TelemetrySink>>) -> Self {
        Self::with_status(table, sinks, Arc::new(StatusStore::new()))
    }

    pub fn with_status(
        table: &MappingTable,
        sinks: Vec<Arc<dyn TelemetrySink>>,
        status: Arc<StatusStore>,
    ) -> Self {
        let topic_by_node = table
            .read
            .iter()
            .map(|m| (m.node_id.clone(), m.topic.clone()))
            .collect();
        let node_by_topic = table
            .write
            .iter()
            .map(|m| (m.topic.clone(), m.node_id.clone()))
            .collect();
        Self {
            topic_by_node,
            node_by_topic,
            sinks,
            status,
        }
    }

    pub fn status(&self) -> Arc<StatusStore> {
        self.status.clone()
    }

    /// Nodes that need an OPC UA subscription — i.e. the read direction.
    pub fn subscribed_nodes(&self) -> impl Iterator<Item = &str> {
        self.topic_by_node.keys().map(|s| s.as_str())
    }

    /// Topics that need an MQTT subscription — i.e. the write direction.
    pub fn subscribed_topics(&self) -> impl Iterator<Item = &str> {
        self.node_by_topic.keys().map(|s| s.as_str())
    }

    pub async fn handle_opcua_change(&self, change: OpcUaChange) {
        let Some(topic) = self.topic_by_node.get(&change.node_id) else {
            return;
        };
        self.status.record(topic, &change.value);
        for sink in &self.sinks {
            if let Err(err) = sink.publish(topic, &change.value).await {
                tracing::warn!(%topic, node_id = %change.node_id, error = %err, "sink publish failed");
            }
        }
    }

    pub async fn handle_mqtt_message(&self, msg: MqttMessage, writer: &dyn OpcUaWriter) {
        let Some(node_id) = self.node_by_topic.get(&msg.topic) else {
            return;
        };
        if let Err(err) = writer.write(node_id, &msg.value).await {
            tracing::warn!(topic = %msg.topic, node_id = %node_id, error = %err, "opc ua write failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::mapping::NodeMapping;
    use crate::sinks::test_support::RecordingSink;
    use std::sync::Mutex;

    fn table() -> MappingTable {
        MappingTable {
            read: vec![NodeMapping {
                node_id: "ns=2;s=A".into(),
                topic: "plant/a".into(),
            }],
            write: vec![NodeMapping {
                node_id: "ns=2;s=B".into(),
                topic: "plant/b/set".into(),
            }],
        }
    }

    #[tokio::test]
    async fn read_change_is_published_to_mapped_topic() {
        let sink = Arc::new(RecordingSink::default());
        let dispatcher = Dispatcher::new(&table(), vec![sink.clone()]);

        dispatcher
            .handle_opcua_change(OpcUaChange {
                node_id: "ns=2;s=A".into(),
                value: Value::Int(42),
            })
            .await;

        let published = sink.published.lock().unwrap();
        assert_eq!(published.len(), 1);
        assert_eq!(published[0], ("plant/a".to_string(), Value::Int(42)));
    }

    #[tokio::test]
    async fn unmapped_node_change_is_ignored() {
        let sink = Arc::new(RecordingSink::default());
        let dispatcher = Dispatcher::new(&table(), vec![sink.clone()]);

        dispatcher
            .handle_opcua_change(OpcUaChange {
                node_id: "ns=2;s=UNKNOWN".into(),
                value: Value::Int(1),
            })
            .await;

        assert!(sink.published.lock().unwrap().is_empty());
    }

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
    async fn mqtt_message_writes_to_mapped_node() {
        let sink = Arc::new(RecordingSink::default());
        let dispatcher = Dispatcher::new(&table(), vec![sink]);
        let writer = RecordingWriter {
            writes: Mutex::new(vec![]),
        };

        dispatcher
            .handle_mqtt_message(
                MqttMessage {
                    topic: "plant/b/set".into(),
                    value: Value::Float(3.5),
                },
                &writer,
            )
            .await;

        let writes = writer.writes.lock().unwrap();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0], ("ns=2;s=B".to_string(), Value::Float(3.5)));
    }

    #[tokio::test]
    async fn mqtt_message_on_unmapped_topic_is_ignored() {
        let sink = Arc::new(RecordingSink::default());
        let dispatcher = Dispatcher::new(&table(), vec![sink]);
        let writer = RecordingWriter {
            writes: Mutex::new(vec![]),
        };

        dispatcher
            .handle_mqtt_message(
                MqttMessage {
                    topic: "plant/unmapped".into(),
                    value: Value::Bool(true),
                },
                &writer,
            )
            .await;

        assert!(writer.writes.lock().unwrap().is_empty());
    }
}
