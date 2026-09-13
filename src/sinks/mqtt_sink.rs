use async_trait::async_trait;
use rumqttc::{AsyncClient, QoS};

use super::{TelemetrySink, Value};

pub struct MqttSink {
    client: AsyncClient,
    qos: QoS,
}

impl MqttSink {
    pub fn new(client: AsyncClient, qos: QoS) -> Self {
        Self { client, qos }
    }
}

#[async_trait]
impl TelemetrySink for MqttSink {
    async fn publish(&self, topic: &str, value: &Value) -> anyhow::Result<()> {
        let payload = serde_json::to_vec(&value.to_json())?;
        self.client
            .publish(topic, self.qos, false, payload)
            .await?;
        Ok(())
    }
}
