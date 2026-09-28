//! An in-process OPC UA server for end-to-end tests, built on
//! `async-opcua-server`. Every call into that crate lives in this one file
//! so that, if its API differs from what's assumed here, the fix is
//! contained. Assumptions worth checking on first build:
//!
//! - `opcua_server::prelude` exposes `ServerBuilder`, `simple_node_manager`,
//!   `NamespaceMetadata`, `SimpleNodeManager`, `VariableBuilder`, `NodeId`,
//!   `DataValue`, `StatusCode` (taken from the crate's server docs).
//! - `ServerBuilder::new()...build()` yields a server with an anonymous,
//!   security-policy-None endpoint; if not, add one via `.add_endpoint(..)`.
//! - `node_manager.inner().add_write_callback(node_id, cb)` exists on the
//!   simple node manager and `VariableBuilder::writable()` marks a node
//!   writable.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use opcua_server::prelude::*;
use opcua_types::Variant;

use super::free_port;

const NAMESPACE_URI: &str = "urn:opcua-to-mqtt:e2e";

pub struct MockOpcUaServer {
    pub port: u16,
    pub ns: u16,
    set_value: Box<dyn Fn(&str, Variant) + Send + Sync>,
    cancel: Box<dyn Fn() + Send + Sync>,
    writes: Arc<Mutex<Vec<(String, Variant)>>>,
}

impl MockOpcUaServer {
    /// Starts a server exposing one writable variable per `(name, initial)`.
    pub async fn start(nodes: Vec<(&'static str, Variant)>) -> Self {
        let port = free_port();

        let (server, handle) = ServerBuilder::new()
            .application_name("opcua-to-mqtt e2e mock")
            .application_uri("urn:opcua-to-mqtt:e2e-mock")
            .host("127.0.0.1")
            .port(port)
            .with_node_manager(simple_node_manager(
                NamespaceMetadata {
                    namespace_uri: NAMESPACE_URI.to_string(),
                    ..Default::default()
                },
                "e2e",
            ))
            .build()
            .expect("mock opc ua: failed to build server");
        let handle = Arc::new(handle);

        let node_manager = handle
            .node_managers()
            .get_of_type::<SimpleNodeManager>()
            .expect("mock opc ua: simple node manager not registered");
        let ns = handle
            .get_namespace_index(NAMESPACE_URI)
            .expect("mock opc ua: namespace not registered");

        let writes: Arc<Mutex<Vec<(String, Variant)>>> = Arc::new(Mutex::new(Vec::new()));
        for (name, initial) in nodes {
            let node_id = NodeId::new(ns, name);
            {
                let address_space = node_manager.address_space();
                let mut address_space = address_space.write();
                VariableBuilder::new(&node_id, name, name)
                    .value(initial)
                    .writable()
                    .insert(&mut address_space);
            }
            let writes = writes.clone();
            let key = name.to_string();
            node_manager
                .inner()
                .add_write_callback(node_id, move |value, _range| {
                    if let Some(variant) = value.value {
                        writes.lock().unwrap().push((key.clone(), variant));
                    }
                    StatusCode::Good
                });
        }

        tokio::spawn(server.run());
        wait_until_listening(port).await;

        let set_value: Box<dyn Fn(&str, Variant) + Send + Sync> = {
            let handle = handle.clone();
            let node_manager = node_manager.clone();
            Box::new(move |name, value| {
                node_manager
                    .set_value(
                        &handle.subscriptions(),
                        &NodeId::new(ns, name),
                        None,
                        DataValue::new_now(value),
                    )
                    .expect("mock opc ua: set_value failed");
            })
        };
        let cancel: Box<dyn Fn() + Send + Sync> = {
            let handle = handle.clone();
            Box::new(move || handle.cancel())
        };

        Self {
            port,
            ns,
            set_value,
            cancel,
            writes,
        }
    }

    /// The node id string as it appears in bridge config, e.g. `ns=2;s=Tag`.
    pub fn node_id(&self, name: &str) -> String {
        format!("ns={};s={}", self.ns, name)
    }

    /// Changes a variable's value server-side, as a PLC would.
    pub fn set(&self, name: &str, value: Variant) {
        (self.set_value)(name, value);
    }

    /// Every value a client (the bridge) has successfully written to `name`.
    pub fn writes_to(&self, name: &str) -> Vec<Variant> {
        self.writes
            .lock()
            .unwrap()
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
            .collect()
    }
}

impl Drop for MockOpcUaServer {
    fn drop(&mut self) {
        (self.cancel)();
    }
}

async fn wait_until_listening(port: u16) {
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("mock opc ua: server never started listening on port {port}");
}
