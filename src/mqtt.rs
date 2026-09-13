use crate::config::MqttConfig;
use log::warn;
use rumqttc::{Client, Connection, MqttOptions, QoS};
use std::thread;
use std::time::Duration;

pub fn connect(cfg: &MqttConfig) -> (Client, Connection) {
    let mut options = MqttOptions::new(&cfg.client_id, &cfg.broker_host, cfg.broker_port);
    options.set_keep_alive(Duration::from_secs(30));
    if let (Some(user), Some(pass)) = (&cfg.username, &cfg.password) {
        options.set_credentials(user, pass);
    }
    Client::new(options, 16)
}

/// Drives the MQTT event loop on a background thread; rumqttc requires the
/// `Connection` to be polled continuously for publishes to actually be sent.
pub fn spawn_event_loop(mut connection: Connection) {
    thread::spawn(move || {
        for notification in connection.iter() {
            if let Err(e) = notification {
                warn!("mqtt event loop error: {e}");
            }
        }
    });
}

pub fn qos(level: u8) -> QoS {
    match level {
        1 => QoS::AtLeastOnce,
        2 => QoS::ExactlyOnce,
        _ => QoS::AtMostOnce,
    }
}
