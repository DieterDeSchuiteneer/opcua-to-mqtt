use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use opcua_client::{ClientBuilder, DataChangeCallback, IdentityToken, MonitoredItem, Session};
use opcua_types::{
    AttributeId, DataValue, EndpointDescription, MessageSecurityMode, MonitoredItemCreateRequest,
    NodeId, TimestampsToReturn, UserTokenPolicy, WriteValue,
};
use tokio::sync::mpsc;

use crate::bridge::dispatch::{OpcUaChange, OpcUaWriter};
use crate::config::OpcUaConfig;
use crate::sinks::Value;

use super::convert::{value_to_variant, variant_to_value};

pub type OpcUaChangeSender = mpsc::Sender<OpcUaChange>;

/// Thin wrapper around an `async-opcua-client` session. Kept small and
/// isolated behind the `OpcUaWriter`/event-channel boundary so the rest of
/// the bridge (dispatch, sinks, config) never touches `opcua_client` types
/// directly — if this crate's API shifts, the fallout is contained here.
pub struct ConnectedClient {
    session: Arc<Session>,
    publishing_interval: Duration,
}

impl ConnectedClient {
    pub async fn connect(
        cfg: &OpcUaConfig,
        username: Option<String>,
        password: Option<String>,
    ) -> Result<Self> {
        let mut client = ClientBuilder::new()
            .application_name("opcua-to-mqtt")
            .application_uri("urn:opcua-to-mqtt")
            .trust_server_certs(true)
            .create_sample_keypair(true)
            .session_retry_limit(3)
            .client()
            .map_err(|errors| anyhow!("invalid OPC UA client configuration: {}", errors.join("; ")))?;

        let security_mode = match cfg.security_mode.as_str() {
            "None" => MessageSecurityMode::None,
            "Sign" => MessageSecurityMode::Sign,
            "SignAndEncrypt" => MessageSecurityMode::SignAndEncrypt,
            other => bail!("invalid opcua.security_mode: {other}"),
        };

        let identity_token = match (username, password) {
            (Some(username), Some(password)) => IdentityToken::new_user_name(username, password),
            _ => IdentityToken::Anonymous,
        };

        let endpoint: EndpointDescription = (
            cfg.endpoint.as_str(),
            cfg.security_policy.as_str(),
            security_mode,
            UserTokenPolicy::anonymous(),
        )
            .into();

        let (session, event_loop) = client
            .connect_to_matching_endpoint(endpoint, identity_token)
            .await
            .with_context(|| format!("connecting to OPC UA endpoint {}", cfg.endpoint))?;

        event_loop.spawn();
        if !session.wait_for_connection().await {
            bail!("could not establish an OPC UA session with {}", cfg.endpoint);
        }

        Ok(Self {
            session,
            publishing_interval: Duration::from_millis(cfg.poll_interval_ms as u64),
        })
    }

    /// Subscribes to every given node id and forwards data changes onto
    /// `sender` for the lifetime of the session.
    pub async fn subscribe(&self, node_ids: &[String], sender: OpcUaChangeSender) -> Result<()> {
        if node_ids.is_empty() {
            return Ok(());
        }

        let mut config_id_by_node: HashMap<NodeId, String> = HashMap::new();
        for raw in node_ids {
            let node_id =
                NodeId::from_str(raw).map_err(|_| anyhow!("invalid node id: {raw}"))?;
            config_id_by_node.insert(node_id, raw.clone());
        }
        let callback_lookup = config_id_by_node.clone();

        let subscription_id = self
            .session
            .create_subscription(
                self.publishing_interval,
                10,
                30,
                0,
                0,
                true,
                DataChangeCallback::new(move |data_value: DataValue, item: &MonitoredItem| {
                    let Some(config_id) = callback_lookup.get(&item.item_to_monitor().node_id)
                    else {
                        return;
                    };
                    let Some(variant) = data_value.value.as_ref() else {
                        return;
                    };
                    let change = OpcUaChange {
                        node_id: config_id.clone(),
                        value: variant_to_value(variant),
                    };
                    if let Err(err) = sender.try_send(change) {
                        tracing::warn!(%err, "dropped OPC UA change: channel full or closed");
                    }
                }),
            )
            .await
            .context("creating OPC UA subscription")?;

        let items_to_create: Vec<MonitoredItemCreateRequest> = config_id_by_node
            .keys()
            .map(|node_id| node_id.clone().into())
            .collect();

        self.session
            .create_monitored_items(subscription_id, TimestampsToReturn::Both, items_to_create)
            .await
            .context("creating OPC UA monitored items")?;

        for node_id in node_ids {
            tracing::info!(%node_id, "subscribed to OPC UA node");
        }

        Ok(())
    }
}

#[async_trait]
impl OpcUaWriter for ConnectedClient {
    async fn write(&self, node_id: &str, value: &Value) -> Result<()> {
        let node_id =
            NodeId::from_str(node_id).map_err(|_| anyhow!("invalid node id: {node_id}"))?;
        let write_value = WriteValue {
            node_id,
            attribute_id: AttributeId::Value as u32,
            value: DataValue::new_now(value_to_variant(value)),
            ..Default::default()
        };
        let results = self
            .session
            .write(&[write_value])
            .await
            .context("OPC UA write request failed")?;
        if let Some(status) = results.first() {
            if status.is_bad() {
                bail!("OPC UA write returned {status}");
            }
        }
        Ok(())
    }
}
