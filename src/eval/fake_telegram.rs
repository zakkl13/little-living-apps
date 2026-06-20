//! A fake Telegram Bot API server for the eval harness (and binary-driven tests): the spawned `lila`
//! binary points `TELEGRAM_API_BASE_URL` here; the harness pushes scripted owner turns (served on
//! `getUpdates`) and captures the `sendMessage` calls the manager makes back — the eval's single
//! substitution ("deliveries are graded, not sent"). A tiny raw-TCP HTTP/1.1 server (requests are
//! small and we control the reqwest client), so there are no web-framework handler-trait subtleties.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone, Default)]
struct Shared {
    pending: Arc<Mutex<VecDeque<Value>>>,
    sent: Arc<Mutex<Vec<(i64, String)>>>,
    seq: Arc<AtomicI64>,
}

/// A running fake Telegram server.
pub struct FakeTelegram {
    base_url: String,
    shared: Shared,
}

impl FakeTelegram {
    /// Start the server on an ephemeral loopback port.
    pub async fn start() -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let shared = Shared::default();
        let accept_shared = shared.clone();
        tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                let s = accept_shared.clone();
                tokio::spawn(async move {
                    let _ = serve_conn(sock, s).await;
                });
            }
        });
        Ok(Self {
            base_url: format!("http://127.0.0.1:{}", addr.port()),
            shared,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Queue an inbound owner message (delivered on the next `getUpdates`).
    pub fn push_owner_message(&self, from_id: i64, chat_id: i64, text: &str) {
        if let Ok(mut q) = self.shared.pending.lock() {
            q.push_back(json!({
                "message": { "text": text, "chat": { "id": chat_id }, "from": { "id": from_id } }
            }));
        }
    }

    /// All `sendMessage` calls so far, as (chat_id, text).
    pub fn sent(&self) -> Vec<(i64, String)> {
        self.shared
            .sent
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default()
    }

    /// The text of every captured delivery (what the owner saw).
    pub fn deliveries(&self) -> Vec<String> {
        self.sent().into_iter().map(|(_, t)| t).collect()
    }
}

async fn serve_conn(mut sock: TcpStream, shared: Shared) -> std::io::Result<()> {
    let (path, body) = read_request(&mut sock).await?;
    let method = path.rsplit('/').next().unwrap_or("").to_string();
    let response = dispatch(&shared, &method, &body).await;
    let payload = response.to_string();
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    sock.write_all(head.as_bytes()).await?;
    sock.write_all(payload.as_bytes()).await?;
    sock.flush().await
}

async fn read_request(sock: &mut TcpStream) -> std::io::Result<(String, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        let n = sock.read(&mut tmp).await?;
        if n == 0 {
            return Ok((String::new(), Vec::new()));
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            break pos;
        }
    };
    let headers = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let path = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("")
        .to_string();
    let content_length = headers
        .lines()
        .find_map(|l| {
            l.to_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse().ok())
        })
        .flatten()
        .unwrap_or(0usize);
    let body_start = header_end + 4;
    let mut body = buf[body_start..].to_vec();
    while body.len() < content_length {
        let n = sock.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(content_length);
    Ok((path, body))
}

async fn dispatch(shared: &Shared, method: &str, body: &[u8]) -> Value {
    match method {
        "getUpdates" => get_updates(shared).await,
        "sendMessage" => send_message(shared, body),
        "sendPhoto" => json!({ "ok": true, "result": { "message_id": 1 } }),
        _ => json!({ "ok": true, "result": true }),
    }
}

async fn get_updates(shared: &Shared) -> Value {
    let drained: Vec<Value> = drain_pending(shared);
    if drained.is_empty() {
        // Mimic a (short) long-poll so the client doesn't busy-loop.
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
    json!({ "ok": true, "result": drained })
}

fn drain_pending(shared: &Shared) -> Vec<Value> {
    let Ok(mut queue) = shared.pending.lock() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    while let Some(mut update) = queue.pop_front() {
        update["update_id"] = json!(shared.seq.fetch_add(1, Ordering::SeqCst) + 1);
        out.push(update);
    }
    out
}

fn send_message(shared: &Shared, body: &[u8]) -> Value {
    let parsed: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let chat_id = parsed.get("chat_id").and_then(Value::as_i64).unwrap_or(0);
    let text = parsed
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if let Ok(mut s) = shared.sent.lock() {
        s.push((chat_id, text));
    }
    json!({ "ok": true, "result": { "message_id": 1 } })
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Block until `sent` contains a message matching `needle`, or `timeout` elapses (test helper).
pub async fn wait_for_sent(tg: &FakeTelegram, needle: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if tg.sent().iter().any(|(_, t)| t.contains(needle)) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}
