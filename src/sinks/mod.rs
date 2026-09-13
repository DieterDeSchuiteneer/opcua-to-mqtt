use async_trait::async_trait;

#[cfg(feature = "influx")]
mod influx;
mod mqtt_sink;

#[cfg(feature = "influx")]
pub use influx::InfluxSink;
pub use mqtt_sink::MqttSink;

/// A bridged OPC UA value, normalized so sinks don't need to know about
/// `opcua_types::Variant` directly.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
}

impl Value {
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Value::Bool(b) => serde_json::Value::Bool(*b),
            Value::Int(i) => serde_json::Value::Number((*i).into()),
            Value::Float(f) => serde_json::Number::from_f64(*f)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
            Value::String(s) => serde_json::Value::String(s.clone()),
        }
    }
}

/// Destination for a bridged OPC UA value change. Implemented by the MQTT
/// publisher (always) and the optional InfluxDB writer.
#[async_trait]
pub trait TelemetrySink: Send + Sync {
    async fn publish(&self, topic: &str, value: &Value) -> anyhow::Result<()>;
}

#[cfg(test)]
pub mod test_support {
    use super::*;
    use std::sync::Mutex;

    /// Records every publish call instead of sending anything over the
    /// network — used to unit test bridge dispatch/filter logic.
    #[derive(Default)]
    pub struct RecordingSink {
        pub published: Mutex<Vec<(String, Value)>>,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_conversion() {
        assert_eq!(Value::Bool(true).to_json(), serde_json::json!(true));
        assert_eq!(Value::Int(42).to_json(), serde_json::json!(42));
        assert_eq!(Value::Float(1.5).to_json(), serde_json::json!(1.5));
        assert_eq!(
            Value::String("hi".into()).to_json(),
            serde_json::json!("hi")
        );
    }
}
