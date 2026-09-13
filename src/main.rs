mod bridge;
mod config;
mod mqtt;

use anyhow::Result;
use std::path::PathBuf;

fn main() -> Result<()> {
    env_logger::init();

    let config_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("config.yaml"));

    let cfg = config::load(&config_path)?;

    let (mqtt_client, connection) = mqtt::connect(&cfg.mqtt);
    mqtt::spawn_event_loop(connection);
    let qos = mqtt::qos(cfg.mqtt.qos);

    log::info!("connecting to OPC UA endpoint {}", cfg.opcua.endpoint);
    bridge::run(&cfg, mqtt_client, qos)
}
