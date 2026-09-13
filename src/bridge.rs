use crate::config::Config;
use anyhow::{anyhow, Result};
use opcua::client::prelude::*;
use rumqttc::{Client, QoS};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

/// Connects to the OPC UA endpoint, subscribes to the configured nodes, and
/// forwards every value change to MQTT. Blocks for the lifetime of the session.
pub fn run(cfg: &Config, mqtt_client: Client, qos: QoS) -> Result<()> {
    let mut client = ClientBuilder::new()
        .application_name("opcua-to-mqtt")
        .application_uri("urn:opcua-to-mqtt")
        .trust_server_certs(true)
        .create_sample_keypair(true)
        .session_retry_limit(3)
        .client()
        .ok_or_else(|| anyhow!("failed to build OPC UA client"))?;

    let endpoint: EndpointDescription = (
        cfg.opcua.endpoint.as_str(),
        "None",
        MessageSecurityMode::None,
        UserTokenPolicy::anonymous(),
    )
        .into();

    let session = client
        .connect_to_endpoint(endpoint, IdentityToken::Anonymous)
        .map_err(|e| anyhow!("connecting to OPC UA endpoint {}: {:?}", cfg.opcua.endpoint, e))?;

    let topic_by_handle: HashMap<u32, String> = cfg
        .opcua
        .subscriptions
        .iter()
        .enumerate()
        .map(|(i, s)| (i as u32, s.topic.clone()))
        .collect();
    let topic_by_handle = Arc::new(topic_by_handle);
    let mqtt_client = Arc::new(Mutex::new(mqtt_client));

    let subscription_id = {
        let topic_by_handle = topic_by_handle.clone();
        let mqtt_client = mqtt_client.clone();
        let mut session = session.write();
        session.create_subscription(
            cfg.opcua.poll_interval_ms as f64,
            10,
            30,
            0,
            0,
            true,
            DataChangeCallback::new(move |items: Vec<MonitoredItemHandle>| {
                for item in items {
                    let Some(topic) = topic_by_handle.get(&item.client_handle) else {
                        continue;
                    };
                    let Some(value) = item.value.value.as_ref() else {
                        continue;
                    };
                    let payload = format!("{value:?}");
                    if let Ok(client) = mqtt_client.lock() {
                        let _ = client.publish(topic, qos, false, payload.into_bytes());
                    }
                }
            }),
        )?
    };

    let items_to_create = cfg
        .opcua
        .subscriptions
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let node_id = NodeId::from_str(&s.node_id)
                .map_err(|_| anyhow!("invalid node id: {}", s.node_id))?;
            Ok(node_id.into_monitored_item_create_request(i as u32, AttributeId::Value))
        })
        .collect::<Result<Vec<MonitoredItemCreateRequest>>>()?;

    {
        let mut session = session.write();
        session.create_monitored_items(subscription_id, TimestampsToReturn::Both, &items_to_create)?;
    }

    for s in &cfg.opcua.subscriptions {
        log::info!("watching {} -> {}", s.node_id, s.topic);
    }

    Session::run(session);
    Ok(())
}
