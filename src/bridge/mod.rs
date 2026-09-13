pub mod dispatch;
pub mod mapping;
pub mod status;

use std::sync::Arc;

use anyhow::{Context, Result};
use rumqttc::QoS;
use tokio::sync::mpsc;

use crate::config::secrets::{DefaultResolver, SecretResolver};
use crate::config::Config;
use crate::mqtt::{drive_event_loop, ConnectedMqtt};
use crate::opcua::ConnectedClient;
use crate::sinks::{MqttSink, TelemetrySink};

use dispatch::{Dispatcher, MqttMessage, OpcUaChange};
use status::StatusStore;

/// Wires config -> secrets -> both connections -> dispatcher, and runs the
/// bridge until the OPC UA change channel closes (i.e. the process is
/// asked to shut down).
pub async fn run(config: Config) -> Result<()> {
    let resolver = build_resolver(&config).await?;

    let opcua_username = config.opcua.identity.as_ref().map(|i| i.username.clone());
    let opcua_password = match &config.opcua.identity {
        Some(identity) => Some(resolver.resolve(&identity.password).await?),
        None => None,
    };
    let mqtt_password = match &config.mqtt.password {
        Some(secret) => Some(resolver.resolve(secret).await?),
        None => None,
    };

    let mapping_table = mapping::build(&config.mappings, &config.filters)?;

    tracing::info!(endpoint = %config.opcua.endpoint, "connecting to OPC UA server");
    let opcua_client = Arc::new(
        ConnectedClient::connect(&config.opcua, opcua_username, opcua_password).await?,
    );

    tracing::info!(broker = %config.mqtt.broker_host, "connecting to MQTT broker");
    let (mqtt_conn, mqtt_event_loop) = ConnectedMqtt::connect(
        &config.mqtt,
        config.mqtt.username.clone(),
        mqtt_password,
    )?;

    let mut sinks: Vec<Arc<dyn TelemetrySink>> =
        vec![Arc::new(MqttSink::new(mqtt_conn.client.clone(), QoS::AtMostOnce))];
    add_influx_sink(&config, &resolver, &mut sinks).await?;

    let status = Arc::new(StatusStore::new());
    let dispatcher = Arc::new(Dispatcher::with_status(&mapping_table, sinks, status.clone()));

    // MQTT -> OPC UA: drive the event loop, subscribe to write topics, and
    // route every incoming publish through the dispatcher.
    let (mqtt_msg_tx, mut mqtt_msg_rx) = mpsc::channel::<MqttMessage>(256);
    tokio::spawn(drive_event_loop(mqtt_event_loop, mqtt_msg_tx));

    let write_topics: Vec<String> = dispatcher.subscribed_topics().map(String::from).collect();
    mqtt_conn.subscribe(&write_topics).await?;

    {
        let dispatcher = dispatcher.clone();
        let opcua_client = opcua_client.clone();
        tokio::spawn(async move {
            while let Some(msg) = mqtt_msg_rx.recv().await {
                dispatcher
                    .handle_mqtt_message(msg, opcua_client.as_ref())
                    .await;
            }
        });
    }

    #[cfg(feature = "dashboard")]
    if config.dashboard.enabled {
        let bind = config.dashboard.bind.clone();
        let status = status.clone();
        tokio::spawn(async move {
            if let Err(err) = crate::dashboard::serve(bind, status).await {
                tracing::error!(%err, "dashboard server stopped");
            }
        });
    }

    // OPC UA -> MQTT (+ other sinks): subscribe to every read-direction
    // node, then route every change through the dispatcher.
    let read_nodes: Vec<String> = dispatcher.subscribed_nodes().map(String::from).collect();
    let (opcua_tx, mut opcua_rx) = mpsc::channel::<OpcUaChange>(256);
    opcua_client.subscribe(&read_nodes, opcua_tx).await?;

    while let Some(change) = opcua_rx.recv().await {
        dispatcher.handle_opcua_change(change).await;
    }

    Ok(())
}

async fn build_resolver(config: &Config) -> Result<DefaultResolver> {
    #[cfg(feature = "aws-secrets")]
    {
        if config.secrets.aws.enabled {
            let backend = crate::config::secrets::AwsSecretsBackend::new(
                config.secrets.aws.region.clone(),
            )
            .await;
            return Ok(DefaultResolver::new(Some(backend)));
        }
        Ok(DefaultResolver::new(None))
    }
    #[cfg(not(feature = "aws-secrets"))]
    {
        let _ = config;
        Ok(DefaultResolver::new())
    }
}

async fn add_influx_sink(
    config: &Config,
    resolver: &DefaultResolver,
    sinks: &mut Vec<Arc<dyn TelemetrySink>>,
) -> Result<()> {
    #[cfg(feature = "influx")]
    {
        if !config.influx.enabled {
            return Ok(());
        }
        let url = config
            .influx
            .url
            .as_ref()
            .context("influx.url is required when influx.enabled")?;
        let org = config
            .influx
            .org
            .as_ref()
            .context("influx.org is required when influx.enabled")?;
        let bucket = config
            .influx
            .bucket
            .as_ref()
            .context("influx.bucket is required when influx.enabled")?;
        let token_ref = config
            .influx
            .token
            .as_ref()
            .context("influx.token is required when influx.enabled")?;
        let token = resolver.resolve(token_ref).await?;
        sinks.push(Arc::new(crate::sinks::InfluxSink::new(url, org, &token, bucket)));
        tracing::info!(%url, %bucket, "InfluxDB sink enabled");
    }
    #[cfg(not(feature = "influx"))]
    {
        if config.influx.enabled {
            anyhow::bail!("influx.enabled is true but this build was compiled without the 'influx' feature");
        }
        let _ = (config, resolver, sinks);
    }
    Ok(())
}
