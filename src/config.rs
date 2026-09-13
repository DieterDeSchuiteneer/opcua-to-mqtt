use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub opcua: OpcUaConfig,
    pub mqtt: MqttConfig,
}

#[derive(Debug, Deserialize)]
pub struct OpcUaConfig {
    pub endpoint: String,
    #[serde(default = "default_poll_interval_ms")]
    pub poll_interval_ms: u32,
    pub subscriptions: Vec<Subscription>,
}

fn default_poll_interval_ms() -> u32 {
    1000
}

#[derive(Debug, Deserialize, Clone)]
pub struct Subscription {
    pub node_id: String,
    pub topic: String,
}

#[derive(Debug, Deserialize)]
pub struct MqttConfig {
    pub broker_host: String,
    #[serde(default = "default_mqtt_port")]
    pub broker_port: u16,
    pub client_id: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub qos: u8,
}

fn default_mqtt_port() -> u16 {
    1883
}

pub fn load(path: &Path) -> Result<Config> {
    let data = std::fs::read_to_string(path)
        .with_context(|| format!("reading config file {}", path.display()))?;
    let cfg: Config = serde_yaml::from_str(&data)
        .with_context(|| format!("parsing config file {}", path.display()))?;
    anyhow::ensure!(
        !cfg.opcua.subscriptions.is_empty(),
        "opcua.subscriptions must have at least one entry"
    );
    Ok(cfg)
}
