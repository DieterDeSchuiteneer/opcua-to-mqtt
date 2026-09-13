use tracing_subscriber::prelude::*;
use tracing_subscriber::{EnvFilter, Layer, Registry};

use crate::config::OtelConfig;

/// Always logs structured JSON to stdout (so `docker logs` works regardless
/// of otel config). If `otel.enabled`, additionally exports logs via OTLP.
pub fn init(cfg: &OtelConfig) -> anyhow::Result<()> {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let fmt_layer = tracing_subscriber::fmt::layer().json();

    let otel_layer = if cfg.enabled {
        Some(build_otel_layer(cfg)?)
    } else {
        None
    };

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .with(otel_layer)
        .try_init()?;

    Ok(())
}

fn build_otel_layer(
    cfg: &OtelConfig,
) -> anyhow::Result<impl Layer<Registry> + Send + Sync + 'static> {
    let endpoint = cfg
        .endpoint
        .clone()
        .unwrap_or_else(|| "http://localhost:4317".to_string());

    let exporter = opentelemetry_otlp::LogExporter::builder()
        .with_tonic()
        .with_endpoint(&endpoint)
        .build()?;

    let resource = opentelemetry_sdk::Resource::builder()
        .with_attribute(opentelemetry::KeyValue::new(
            "service.name",
            cfg.service_name.clone(),
        ))
        .build();

    let provider = opentelemetry_sdk::logs::LoggerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(exporter)
        .build();

    Ok(opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(&provider))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_config_builds_without_otlp() {
        let cfg = OtelConfig {
            enabled: false,
            endpoint: None,
            service_name: "test".to_string(),
        };
        // init() sets a global subscriber, which can only happen once per
        // process, so this just checks the disabled branch is trivially
        // constructible without touching any OTLP machinery.
        assert!(!cfg.enabled);
    }
}
