use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use opcua_client::prelude::*;
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
    // Assumed to already be an `Arc<Session>` per async-opcua's design for
    // sharing a session across the event loop task and callers issuing
    // writes concurrently; adjust if `connect_to_matching_endpoint` returns
    // a bare `Session` once this is built against the real crate.
    session: Arc<Session>,
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
            .ok_or_else(|| anyhow!("failed to build OPC UA client"))?;

        let security_mode = match cfg.security_mode.as_str() {
            "None" => MessageSecurityMode::None,
            "Sign" => MessageSecurityMode::Sign,
            "SignAndEncrypt" => MessageSecurityMode::SignAndEncrypt,
            other => anyhow::bail!("invalid opcua.security_mode: {other}"),
        };

        let identity_token = match (username, password) {
            (Some(username), Some(password)) => IdentityToken::UserName(username, password),
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

        tokio::spawn(event_loop.run());

        Ok(Self { session })
    }

    /// Subscribes to every given node id and forwards data changes onto
    /// `sender` for the lifetime of the session.
    pub async fn subscribe(&self, node_ids: &[String], sender: OpcUaChangeSender) -> Result<()> {
        if node_ids.is_empty() {
            return Ok(());
        }

        let node_by_handle: HashMap<u32, String> = node_ids
            .iter()
            .enumerate()
            .map(|(i, n)| (i as u32 + 1, n.clone()))
            .collect();
        let callback_handles = node_by_handle.clone();

        let subscription_id = self
            .session
            .create_subscription(
                Duration::from_millis(1000),
                10,
                30,
                0,
                0,
                true,
                DataChangeCallback::new(move |items: Vec<MonitoredItemHandle>| {
                    for item in items {
                        let Some(node_id) = callback_handles.get(&item.client_handle) else {
                            continue;
                        };
                        let Some(value) = item.value.value.as_ref() else {
                            continue;
                        };
                        let change = OpcUaChange {
                            node_id: node_id.clone(),
                            value: variant_to_value(value),
                        };
                        if let Err(err) = sender.try_send(change) {
                            tracing::warn!(%err, "dropped OPC UA change: channel full or closed");
                        }
                    }
                }),
            )
            .await
            .context("creating OPC UA subscription")?;

        let items_to_create = node_by_handle
            .iter()
            .map(|(handle, node_id)| {
                let node_id = NodeId::from_str(node_id)
                    .map_err(|_| anyhow!("invalid node id: {node_id}"))?;
                Ok(MonitoredItemCreateRequest {
                    item_to_monitor: ReadValueId {
                        node_id,
                        attribute_id: AttributeId::Value as u32,
                        index_range: UAString::null(),
                        data_encoding: QualifiedName::null(),
                    },
                    monitoring_mode: MonitoringMode::Reporting,
                    requested_parameters: MonitoringParameters {
                        client_handle: *handle,
                        sampling_interval: -1.0,
                        filter: ExtensionObject::null(),
                        queue_size: 1,
                        discard_oldest: true,
                    },
                })
            })
            .collect::<Result<Vec<MonitoredItemCreateRequest>>>()?;

        self.session
            .create_monitored_items(subscription_id, TimestampsToReturn::Both, &items_to_create)
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
            index_range: UAString::null(),
            value: DataValue::new_now(value_to_variant(value)),
        };
        let results = self
            .session
            .write(&[write_value])
            .await
            .context("OPC UA write request failed")?;
        if let Some(status) = results.first() {
            if status.is_bad() {
                anyhow::bail!("OPC UA write returned {status}");
            }
        }
        Ok(())
    }
}
