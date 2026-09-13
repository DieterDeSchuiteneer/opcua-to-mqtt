use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::{Json, Router};

use crate::bridge::status::StatusStore;

/// Read-only status dashboard: connection/mapping health, not a config
/// editor. No auth is built in — bind this to an internal network or
/// localhost only.
pub async fn serve(bind: String, status: Arc<StatusStore>) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/", get(status_page))
        .route("/status.json", get(status_json))
        .with_state(status);

    let addr: SocketAddr = bind.parse()?;
    tracing::info!(%addr, "dashboard listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn healthz() -> &'static str {
    "ok"
}

async fn status_json(State(status): State<Arc<StatusStore>>) -> impl IntoResponse {
    let rows: Vec<_> = status
        .snapshot()
        .into_iter()
        .map(|(topic, value, seen)| {
            serde_json::json!({
                "topic": topic,
                "value": value.to_json(),
                "last_seen": format!("{seen:?}"),
            })
        })
        .collect();
    Json(rows)
}

async fn status_page(State(status): State<Arc<StatusStore>>) -> Html<String> {
    let mut rows = String::new();
    for (topic, value, seen) in status.snapshot() {
        rows.push_str(&format!(
            "<tr><td>{}</td><td>{:?}</td><td>{:?}</td></tr>",
            html_escape(&topic),
            value,
            seen
        ));
    }
    Html(format!(
        "<html><head><title>opcua-to-mqtt</title></head><body>\
         <h1>opcua-to-mqtt status</h1>\
         <table border=\"1\" cellpadding=\"4\">\
         <tr><th>Topic</th><th>Last value</th><th>Last seen</th></tr>{rows}</table>\
         </body></html>"
    ))
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
