use async_trait::async_trait;
use futures_util::stream;
use influxdb2::models::DataPoint;
use influxdb2::Client as InfluxClient;

use super::{TelemetrySink, Value};

pub struct InfluxSink {
    client: InfluxClient,
    bucket: String,
}

impl InfluxSink {
    pub fn new(url: &str, org: &str, token: &str, bucket: &str) -> Self {
        Self {
            client: InfluxClient::new(url, org, token),
            bucket: bucket.to_string(),
        }
    }
}

#[async_trait]
impl TelemetrySink for InfluxSink {
    async fn publish(&self, topic: &str, value: &Value) -> anyhow::Result<()> {
        let mut builder = DataPoint::builder("opcua_to_mqtt").tag("topic", topic);
        builder = match value {
            Value::Bool(b) => builder.field("value", *b),
            Value::Int(i) => builder.field("value", *i),
            Value::Float(f) => builder.field("value", *f),
            Value::String(s) => builder.field("value", s.clone()),
        };
        let point = builder.build()?;
        self.client
            .write(&self.bucket, stream::iter(vec![point]))
            .await?;
        Ok(())
    }
}
