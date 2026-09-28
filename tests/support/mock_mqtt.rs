//! A minimal in-process MQTT 3.1.1 broker for end-to-end tests. It speaks
//! just enough of the protocol for rumqttc: CONNECT, SUBSCRIBE, PUBLISH
//! (QoS 0/1), PINGREQ and DISCONNECT. Like a real broker it echoes a
//! publish back to *every* matching subscriber, including the publisher.
//!
//! Tests observe it via `published()` / `subscribed_filters()` /
//! `credentials()` and act as an external publisher via `inject()`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::OwnedReadHalf;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::eventually;

#[derive(Default)]
struct State {
    subscribers: Vec<Subscriber>,
    published: Vec<(String, Vec<u8>)>,
    credentials: Vec<(Option<String>, Option<String>)>,
    next_conn_id: u64,
}

struct Subscriber {
    conn_id: u64,
    filters: Vec<String>,
    tx: mpsc::UnboundedSender<Vec<u8>>,
}

pub struct MockMqttBroker {
    pub port: u16,
    state: Arc<Mutex<State>>,
    accept_task: JoinHandle<()>,
}

impl MockMqttBroker {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock mqtt: bind");
        let port = listener.local_addr().unwrap().port();
        let state = Arc::new(Mutex::new(State::default()));

        let accept_state = state.clone();
        let accept_task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                tokio::spawn(handle_connection(socket, accept_state.clone()));
            }
        });

        Self {
            port,
            state,
            accept_task,
        }
    }

    /// Every (topic, payload-as-string) a client has published to the broker.
    pub fn published(&self) -> Vec<(String, String)> {
        self.state
            .lock()
            .unwrap()
            .published
            .iter()
            .map(|(t, p)| (t.clone(), String::from_utf8_lossy(p).to_string()))
            .collect()
    }

    pub fn published_on(&self, topic: &str) -> Vec<String> {
        self.published()
            .into_iter()
            .filter(|(t, _)| t == topic)
            .map(|(_, p)| p)
            .collect()
    }

    pub fn subscribed_filters(&self) -> Vec<String> {
        self.state
            .lock()
            .unwrap()
            .subscribers
            .iter()
            .flat_map(|s| s.filters.clone())
            .collect()
    }

    /// (username, password) pairs presented in CONNECT packets.
    pub fn credentials(&self) -> Vec<(Option<String>, Option<String>)> {
        self.state.lock().unwrap().credentials.clone()
    }

    /// Acts as an external MQTT publisher: delivers to matching subscribers.
    pub fn inject(&self, topic: &str, payload: &[u8]) {
        deliver(&self.state, topic, payload);
    }

    pub async fn wait_for_subscription(&self, filter: &str) {
        eventually(
            &format!("a subscription to '{filter}'"),
            Duration::from_secs(10),
            || self.subscribed_filters().iter().any(|f| f == filter),
        )
        .await;
    }

    pub async fn wait_for_publish(&self, topic: &str, payload: &str) {
        eventually(
            &format!("publish '{payload}' on '{topic}'"),
            Duration::from_secs(10),
            || self.published_on(topic).iter().any(|p| p == payload),
        )
        .await;
    }
}

impl Drop for MockMqttBroker {
    fn drop(&mut self) {
        self.accept_task.abort();
    }
}

async fn handle_connection(socket: TcpStream, state: Arc<Mutex<State>>) {
    let (mut reader, mut writer) = socket.into_split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();

    let writer_task = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            if writer.write_all(&frame).await.is_err() {
                break;
            }
        }
    });

    let conn_id = {
        let mut s = state.lock().unwrap();
        s.next_conn_id += 1;
        s.next_conn_id
    };

    while let Some((header, body)) = read_packet(&mut reader).await {
        match header >> 4 {
            // CONNECT
            1 => {
                if let Some(creds) = parse_connect_credentials(&body) {
                    state.lock().unwrap().credentials.push(creds);
                }
                let _ = tx.send(vec![0x20, 0x02, 0x00, 0x00]);
            }
            // PUBLISH
            3 => {
                let qos = (header >> 1) & 0x03;
                let mut pos = 0;
                let Some(topic) = read_str(&body, &mut pos) else {
                    continue;
                };
                if qos > 0 {
                    let Some(id) = body.get(pos..pos + 2) else {
                        continue;
                    };
                    let _ = tx.send(vec![0x40, 0x02, id[0], id[1]]);
                    pos += 2;
                }
                let payload = body[pos..].to_vec();
                state
                    .lock()
                    .unwrap()
                    .published
                    .push((topic.clone(), payload.clone()));
                deliver(&state, &topic, &payload);
            }
            // SUBSCRIBE
            8 => {
                let Some(packet_id) = body.get(0..2) else {
                    continue;
                };
                let packet_id = [packet_id[0], packet_id[1]];
                let mut pos = 2;
                let mut filters = Vec::new();
                let mut granted = Vec::new();
                while pos < body.len() {
                    let Some(filter) = read_str(&body, &mut pos) else {
                        break;
                    };
                    let Some(requested) = body.get(pos).copied() else {
                        break;
                    };
                    pos += 1;
                    filters.push(filter);
                    granted.push(requested.min(1));
                }
                {
                    let mut s = state.lock().unwrap();
                    match s.subscribers.iter_mut().find(|sub| sub.conn_id == conn_id) {
                        Some(sub) => sub.filters.extend(filters),
                        None => s.subscribers.push(Subscriber {
                            conn_id,
                            filters,
                            tx: tx.clone(),
                        }),
                    }
                }
                let mut suback = vec![0x90];
                encode_remaining_length(2 + granted.len(), &mut suback);
                suback.extend_from_slice(&packet_id);
                suback.extend(granted);
                let _ = tx.send(suback);
            }
            // PINGREQ
            12 => {
                let _ = tx.send(vec![0xD0, 0x00]);
            }
            // DISCONNECT
            14 => break,
            _ => {}
        }
    }

    state
        .lock()
        .unwrap()
        .subscribers
        .retain(|s| s.conn_id != conn_id);
    writer_task.abort();
}

fn deliver(state: &Mutex<State>, topic: &str, payload: &[u8]) {
    let mut body = Vec::new();
    body.extend_from_slice(&(topic.len() as u16).to_be_bytes());
    body.extend_from_slice(topic.as_bytes());
    body.extend_from_slice(payload);
    let mut frame = vec![0x30];
    encode_remaining_length(body.len(), &mut frame);
    frame.extend(body);

    let s = state.lock().unwrap();
    for sub in &s.subscribers {
        if sub.filters.iter().any(|f| topic_matches(f, topic)) {
            let _ = sub.tx.send(frame.clone());
        }
    }
}

fn topic_matches(filter: &str, topic: &str) -> bool {
    let mut filter_parts = filter.split('/');
    let mut topic_parts = topic.split('/');
    loop {
        match (filter_parts.next(), topic_parts.next()) {
            (Some("#"), _) => return true,
            (Some("+"), Some(_)) => {}
            (Some(f), Some(t)) if f == t => {}
            (None, None) => return true,
            _ => return false,
        }
    }
}

async fn read_packet(reader: &mut OwnedReadHalf) -> Option<(u8, Vec<u8>)> {
    let header = reader.read_u8().await.ok()?;
    let mut len = 0usize;
    let mut shift = 0;
    loop {
        let byte = reader.read_u8().await.ok()?;
        len |= ((byte & 0x7f) as usize) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift > 21 {
            return None;
        }
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).await.ok()?;
    Some((header, body))
}

fn encode_remaining_length(mut len: usize, out: &mut Vec<u8>) {
    loop {
        let mut byte = (len % 128) as u8;
        len /= 128;
        if len > 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if len == 0 {
            break;
        }
    }
}

fn read_bytes(buf: &[u8], pos: &mut usize) -> Option<Vec<u8>> {
    let len_bytes = buf.get(*pos..*pos + 2)?;
    let len = u16::from_be_bytes([len_bytes[0], len_bytes[1]]) as usize;
    *pos += 2;
    let bytes = buf.get(*pos..*pos + len)?.to_vec();
    *pos += len;
    Some(bytes)
}

fn read_str(buf: &[u8], pos: &mut usize) -> Option<String> {
    read_bytes(buf, pos).map(|b| String::from_utf8_lossy(&b).to_string())
}

fn parse_connect_credentials(body: &[u8]) -> Option<(Option<String>, Option<String>)> {
    let mut pos = 0;
    read_bytes(body, &mut pos)?; // protocol name
    pos += 1; // protocol level
    let flags = *body.get(pos)?;
    pos += 1;
    pos += 2; // keep-alive
    read_bytes(body, &mut pos)?; // client id
    if flags & 0x04 != 0 {
        read_bytes(body, &mut pos)?; // will topic
        read_bytes(body, &mut pos)?; // will message
    }
    let username = if flags & 0x80 != 0 {
        Some(read_str(body, &mut pos)?)
    } else {
        None
    };
    let password = if flags & 0x40 != 0 {
        Some(read_str(body, &mut pos)?)
    } else {
        None
    };
    Some((username, password))
}
