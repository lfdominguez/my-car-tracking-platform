//! Email notifications against a minimal in-process SMTP server.
//! Requires DATABASE_URL pointing at Postgres+PostGIS.

mod common;

use std::time::Duration;

use common::{login, start_server};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// Accepts SMTP sessions and forwards each message's DATA section.
async fn fake_smtp() -> (u16, mpsc::UnboundedReceiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            let tx = tx.clone();
            tokio::spawn(async move {
                let (r, mut w) = sock.into_split();
                let mut lines = BufReader::new(r).lines();
                w.write_all(b"220 fake ESMTP\r\n").await.unwrap();
                while let Ok(Some(line)) = lines.next_line().await {
                    let cmd = line.to_ascii_uppercase();
                    if cmd.starts_with("EHLO") || cmd.starts_with("HELO") {
                        w.write_all(b"250 fake\r\n").await.unwrap();
                    } else if cmd.starts_with("DATA") {
                        w.write_all(b"354 go\r\n").await.unwrap();
                        let mut data = String::new();
                        while let Ok(Some(l)) = lines.next_line().await {
                            if l == "." {
                                break;
                            }
                            data.push_str(&l);
                            data.push('\n');
                        }
                        let _ = tx.send(data);
                        w.write_all(b"250 queued\r\n").await.unwrap();
                    } else if cmd.starts_with("QUIT") {
                        w.write_all(b"221 bye\r\n").await.unwrap();
                        return;
                    } else {
                        w.write_all(b"250 ok\r\n").await.unwrap();
                    }
                }
            });
        }
    });
    (port, rx)
}

#[tokio::test]
async fn opted_in_users_get_notifications_by_email() {
    let (port, mut rx) = fake_smtp().await;
    // SAFETY: set before the mailer is first used; no other test in this binary
    // reads these variables.
    unsafe {
        std::env::set_var("SMTP_URL", format!("smtp://127.0.0.1:{port}"));
        std::env::set_var("SMTP_FROM", "Car Tracking <noreply@example.com>");
    }
    let Some(base) = start_server().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let user = login(&base).await;

    let cfg: serde_json::Value = user
        .client
        .get(format!("{base}/api/push/config"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cfg["email_enabled"], true);

    // Off by default: nothing is sent.
    user.client
        .post(format!("{base}/api/push/test"))
        .send()
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(500), rx.recv())
            .await
            .is_err(),
        "email must be opt-in"
    );

    let resp = user
        .client
        .put(format!("{base}/api/me/notification-prefs"))
        .json(&json!({ "push": true, "email": true, "muted": [] }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    user.client
        .post(format!("{base}/api/push/test"))
        .send()
        .await
        .unwrap();
    let mail = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("no email arrived")
        .unwrap();
    assert!(
        mail.contains("Subject: Notifications are working"),
        "{mail}"
    );
    assert!(mail.contains("/app"), "{mail}");

    // A muted kind is not emailed either.
    user.client
        .put(format!("{base}/api/me/notification-prefs"))
        .json(&json!({ "push": true, "email": true, "muted": ["test"] }))
        .send()
        .await
        .unwrap();
    user.client
        .post(format!("{base}/api/push/test"))
        .send()
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(500), rx.recv())
            .await
            .is_err()
    );
}
