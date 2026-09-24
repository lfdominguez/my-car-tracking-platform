//! A scripted one-response-per-connection HTTP server for client tests.
//!
//! Just enough HTTP/1.1 to stand in for OpenRouter: every connection gets the next
//! scripted response and is closed, so the client never reuses a socket and each
//! request maps to exactly one script entry.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct Scripted {
    pub status: u16,
    pub headers: Vec<(&'static str, String)>,
    /// Written in order, with `chunk_delay` between them, to exercise streaming.
    pub chunks: Vec<Vec<u8>>,
    pub chunk_delay: Duration,
}

impl Scripted {
    pub fn json(status: u16, body: &str) -> Self {
        Self {
            status,
            headers: vec![("Content-Type", "application/json".into())],
            chunks: vec![body.as_bytes().to_vec()],
            chunk_delay: Duration::ZERO,
        }
    }

    pub fn sse(frames: &[&str]) -> Self {
        Self {
            status: 200,
            headers: vec![("Content-Type", "text/event-stream".into())],
            chunks: frames.iter().map(|f| f.as_bytes().to_vec()).collect(),
            chunk_delay: Duration::from_millis(5),
        }
    }

    pub fn header(mut self, name: &'static str, value: &str) -> Self {
        self.headers.push((name, value.into()));
        self
    }
}

pub struct MockServer {
    pub endpoint: String,
    hits: Arc<AtomicUsize>,
    bodies: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl MockServer {
    pub fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }

    /// Parsed JSON request bodies, in arrival order.
    pub async fn bodies(&self) -> Vec<serde_json::Value> {
        self.bodies.lock().await.clone()
    }
}

/// Serve `script` in order; requests past its end get a 500.
pub async fn serve(script: Vec<Scripted>) -> MockServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let script = Arc::new(script);
    {
        let hits = Arc::clone(&hits);
        let bodies = Arc::clone(&bodies);
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let n = hits.fetch_add(1, Ordering::SeqCst);
                let response = script
                    .get(n)
                    .cloned()
                    .unwrap_or_else(|| Scripted::json(500, r#"{"error":"script exhausted"}"#));
                let bodies = Arc::clone(&bodies);
                tokio::spawn(async move {
                    if let Some(body) = read_request(&mut sock).await {
                        bodies.lock().await.push(body);
                    }
                    let mut head = format!(
                        "HTTP/1.1 {} Scripted\r\nConnection: close\r\n",
                        response.status
                    );
                    for (k, v) in &response.headers {
                        head.push_str(&format!("{k}: {v}\r\n"));
                    }
                    head.push_str("\r\n");
                    let _ = sock.write_all(head.as_bytes()).await;
                    for chunk in &response.chunks {
                        let _ = sock.write_all(chunk).await;
                        let _ = sock.flush().await;
                        tokio::time::sleep(response.chunk_delay).await;
                    }
                    let _ = sock.shutdown().await;
                });
            }
        });
    }
    MockServer {
        endpoint: format!("http://{addr}/chat/completions"),
        hits,
        bodies,
    }
}

async fn read_request(sock: &mut tokio::net::TcpStream) -> Option<serde_json::Value> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        let n = sock.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_ascii_lowercase();
    let len: usize = head
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    while buf.len() < header_end + len {
        let n = sock.read(&mut tmp).await.ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    serde_json::from_slice(&buf[header_end..]).ok()
}
