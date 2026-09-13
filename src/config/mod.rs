pub mod filter;
pub mod secrets;

use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

pub use filter::FilterConfig;
pub use secrets::SecretRef;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub opcua: OpcUaConfig,
    pub mqtt: MqttConfig,
    pub mappings: Vec<Mapping>,
    #[serde(default)]
    pub filters: FilterConfig,
    #[serde(default)]
    pub otel: OtelConfig,
    #[serde(default)]
    pub influx: InfluxConfig,
    #[serde(default)]
    pub dashboard: DashboardConfig,
    #[serde(default)]
    pub secrets: SecretsConfig,
}

#[derive(Debug, Deserialize)]
pub struct OpcUaConfig {
    pub endpoint: String,
    #[serde(default = "default_security_policy")]
    pub security_policy: String,
    #[serde(default = "default_security_mode")]
    pub security_mode: String,
    #[serde(default)]
    pub identity: Option<Identity>,
    #[serde(default = "default_poll_interval_ms")]
    pub poll_interval_ms: u32,
}

fn default_security_policy() -> String {
    "None".to_string()
}

fn default_security_mode() -> String {
    "None".to_string()
}

fn default_poll_interval_ms() -> u32 {
    1000
}

#[derive(Debug, Deserialize)]
pub struct Identity {
    pub username: String,
    pub password: SecretRef,
}

#[derive(Debug, Deserialize)]
pub struct MqttConfig {
    pub broker_host: String,
    #[serde(default = "default_mqtt_port")]
    pub broker_port: u16,
    #[serde(default = "default_client_id")]
    pub client_id: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<SecretRef>,
    #[serde(default)]
    pub tls: MqttTlsConfig,
}

fn default_mqtt_port() -> u16 {
    1883
}

fn default_client_id() -> String {
    "opcua-to-mqtt".to_string()
}

#[derive(Debug, Deserialize, Default)]
pub struct MqttTlsConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub ca_cert: Option<String>,
    #[serde(default)]
    pub client_cert: Option<String>,
    #[serde(default)]
    pub client_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Read,
    Write,
    Both,
}

impl Direction {
    pub fn is_read(self) -> bool {
        matches!(self, Direction::Read | Direction::Both)
    }

    pub fn is_write(self) -> bool {
        matches!(self, Direction::Write | Direction::Both)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Mapping {
    pub node_id: String,
    pub topic: String,
    #[serde(default = "default_direction")]
    pub direction: Direction,
}

fn default_direction() -> Direction {
    Direction::Read
}

#[derive(Debug, Deserialize, Default)]
pub struct OtelConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default = "default_service_name")]
    pub service_name: String,
}

fn default_service_name() -> String {
    "opcua-to-mqtt".to_string()
}

#[derive(Debug, Deserialize, Default)]
pub struct InfluxConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub org: Option<String>,
    #[serde(default)]
    pub bucket: Option<String>,
    #[serde(default)]
    pub token: Option<SecretRef>,
}

#[derive(Debug, Deserialize, Default)]
pub struct DashboardConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_dashboard_bind")]
    pub bind: String,
}

fn default_dashboard_bind() -> String {
    "127.0.0.1:8080".to_string()
}

#[derive(Debug, Deserialize, Default)]
pub struct SecretsConfig {
    #[serde(default)]
    pub aws: AwsSecretsConfig,
}

#[derive(Debug, Deserialize, Default)]
pub struct AwsSecretsConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub region: Option<String>,
}

pub fn load(path: &Path) -> Result<Config> {
    let data = std::fs::read_to_string(path)
        .with_context(|| format!("reading config file {}", path.display()))?;
    let cfg: Config = serde_yaml::from_str(&data)
        .with_context(|| format!("parsing config file {}", path.display()))?;

    anyhow::ensure!(!cfg.opcua.endpoint.is_empty(), "opcua.endpoint is required");
    anyhow::ensure!(!cfg.mqtt.broker_host.is_empty(), "mqtt.broker_host is required");
    anyhow::ensure!(!cfg.mappings.is_empty(), "at least one mapping is required");

    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_config() {
        let yaml = r#"
opcua:
  endpoint: "opc.tcp://localhost:4840"
mqtt:
  broker_host: "localhost"
mappings:
  - node_id: "ns=2;s=Tag1"
    topic: "plant/tag1"
"#;
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.opcua.security_policy, "None");
        assert_eq!(cfg.mqtt.broker_port, 1883);
        assert_eq!(cfg.mappings[0].direction, Direction::Read);
        assert!(!cfg.otel.enabled);
        assert!(!cfg.dashboard.enabled);
    }

    #[test]
    fn parses_full_config_with_secret_refs() {
        let yaml = r#"
opcua:
  endpoint: "opc.tcp://localhost:4840"
  security_policy: Basic256Sha256
  security_mode: SignAndEncrypt
  identity:
    username: "opcuser"
    password: "env:OPCUA_PASSWORD"
mqtt:
  broker_host: "localhost"
  broker_port: 8883
  tls:
    enabled: true
  password: "aws-secret:prod/mqtt#password"
mappings:
  - node_id: "ns=2;s=Tag1"
    topic: "plant/tag1"
    direction: write
filters:
  allow: ["plant/*"]
otel:
  enabled: true
  endpoint: "http://otel:4317"
"#;
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            cfg.opcua.identity.as_ref().unwrap().password,
            SecretRef::Env("OPCUA_PASSWORD".to_string())
        );
        assert_eq!(
            cfg.mqtt.password.unwrap(),
            SecretRef::AwsSecret {
                secret_id: "prod/mqtt".to_string(),
                field: "password".to_string(),
            }
        );
        assert_eq!(cfg.mappings[0].direction, Direction::Write);
        assert!(cfg.otel.enabled);
    }
}
