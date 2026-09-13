use std::fs;
use std::time::Duration;

use anyhow::{Context, Result};
use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, QoS, TlsConfiguration, Transport};
use tokio::sync::mpsc;

use crate::bridge::dispatch::MqttMessage;
use crate::config::MqttConfig;
use crate::sinks::Value;

pub struct ConnectedMqtt {
    pub client: AsyncClient,
}

impl ConnectedMqtt {
    pub fn connect(
        cfg: &MqttConfig,
        username: Option<String>,
        password: Option<String>,
    ) -> Result<(Self, EventLoop)> {
        let mut options = MqttOptions::new(&cfg.client_id, &cfg.broker_host, cfg.broker_port);
        options.set_keep_alive(Duration::from_secs(30));

        if let (Some(user), Some(pass)) = (username, password) {
            options.set_credentials(user, pass);
        }

        if cfg.tls.enabled {
            let ca = cfg
                .tls
                .ca_cert
                .as_ref()
                .map(fs::read)
                .transpose()
                .context("reading mqtt.tls.ca_cert")?
                .unwrap_or_default();
            let client_auth = match (&cfg.tls.client_cert, &cfg.tls.client_key) {
                (Some(cert_path), Some(key_path)) => Some((
                    fs::read(cert_path).context("reading mqtt.tls.client_cert")?,
                    fs::read(key_path).context("reading mqtt.tls.client_key")?,
                )),
                _ => None,
            };
            options.set_transport(Transport::Tls(TlsConfiguration::Simple {
                ca,
                alpn: None,
                client_auth,
            }));
        }

        let (client, event_loop) = AsyncClient::new(options, 64);
        Ok((Self { client }, event_loop))
    }

    pub async fn subscribe(&self, topics: &[String]) -> Result<()> {
        for topic in topics {
            self.client
                .subscribe(topic, QoS::AtLeastOnce)
                .await
                .with_context(|| format!("subscribing to mqtt topic {topic}"))?;
            tracing::info!(%topic, "subscribed to MQTT write-direction topic");
        }
        Ok(())
    }
}

/// Drives the rumqttc event loop (required for publishes/subscriptions to
/// actually flow) and forwards incoming publishes onto `sender`. Runs until
/// the channel closes.
pub async fn drive_event_loop(mut event_loop: EventLoop, sender: mpsc::Sender<MqttMessage>) {
    loop {
        match event_loop.poll().await {
            Ok(Event::Incoming(Packet::Publish(publish))) => {
                let msg = MqttMessage {
                    topic: publish.topic,
                    value: payload_to_value(&publish.payload),
                };
                if sender.send(msg).await.is_err() {
                    break;
                }
            }
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(%err, "mqtt event loop error, retrying");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

fn payload_to_value(payload: &[u8]) -> Value {
    let text = String::from_utf8_lossy(payload);
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(json) => json_to_value(json),
        Err(_) => Value::String(text.to_string()),
    }
}

fn json_to_value(json: serde_json::Value) -> Value {
    match json {
        serde_json::Value::Bool(b) => Value::Bool(b),
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(i) => Value::Int(i),
            None => Value::Float(n.as_f64().unwrap_or_default()),
        },
        serde_json::Value::String(s) => Value::String(s),
        other => Value::String(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json_scalar_payloads() {
        assert_eq!(payload_to_value(b"true"), Value::Bool(true));
        assert_eq!(payload_to_value(b"42"), Value::Int(42));
        assert_eq!(payload_to_value(b"1.5"), Value::Float(1.5));
        assert_eq!(payload_to_value(b"\"hi\""), Value::String("hi".into()));
    }

    #[test]
    fn falls_back_to_raw_string_for_non_json() {
        assert_eq!(payload_to_value(b"not json"), Value::String("not json".into()));
    }
}
