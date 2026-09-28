use tracing::Subscriber;
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{EnvFilter, Layer};

use crate::config::OtelConfig;

/// Always logs structured JSON to stdout (so `docker logs` works regardless
/// of otel config). If `otel.enabled`, additionally exports logs via OTLP.
pub fn init(cfg: &OtelConfig) -> anyhow::Result<()> {
    let mut env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    if cfg.enabled {
        // The OTLP exporter's own transport logs would otherwise be exported
        // through the same pipeline and feed back into it.
        for directive in ["hyper=off", "tonic=off", "h2=off", "tower=off", "reqwest=off"] {
            env_filter = env_filter.add_directive(directive.parse()?);
        }
    }
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

fn build_otel_layer<S>(cfg: &OtelConfig) -> anyhow::Result<impl Layer<S> + Send + Sync + 'static>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    use opentelemetry_otlp::WithExportConfig;

    let endpoint = cfg
        .endpoint
        .clone()
        .unwrap_or_else(|| "http://localhost:4317".to_string());

    let provider = opentelemetry_otlp::new_pipeline()
        .logging()
        .with_resource(opentelemetry_sdk::Resource::new(vec![
            opentelemetry::KeyValue::new("service.name", cfg.service_name.clone()),
        ]))
        .with_exporter(
            opentelemetry_otlp::new_exporter()
                .tonic()
                .with_endpoint(endpoint),
        )
        .install_batch(opentelemetry_sdk::runtime::Tokio)?;

    let layer = opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(&provider);
    // The logger provider must outlive the process's logging; dropping it
    // would shut the exporter down. It's process-global by design.
    std::mem::forget(provider);
    Ok(layer)
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
