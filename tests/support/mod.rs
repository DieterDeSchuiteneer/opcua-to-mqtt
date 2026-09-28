#![allow(dead_code)]

pub mod mock_mqtt;
pub mod mock_opcua;

use std::time::Duration;

/// A currently-free loopback TCP port (released again before returning, so
/// there's a small race window; fine for tests).
pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .unwrap()
        .port()
}

/// Polls `condition` until it holds, panicking with a description if it
/// doesn't within `timeout`. `condition` must be a quick synchronous check.
pub async fn eventually(what: &str, timeout: Duration, condition: impl Fn() -> bool) {
    let start = tokio::time::Instant::now();
    while start.elapsed() < timeout {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(condition(), "timed out after {timeout:?} waiting for {what}");
}
