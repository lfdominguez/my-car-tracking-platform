//! Optional email delivery for notifications, over SMTP.
//!
//! Off unless `SMTP_URL` is set, e.g. `smtps://user:pass@smtp.example.com:465` or
//! `smtp://user:pass@smtp.example.com:587?tls=required`. `SMTP_FROM` is the sender
//! (`Car Tracking <noreply@example.com>`). Users opt in per account with
//! `notification_prefs.email`.

use std::sync::LazyLock;

use lettre::message::Mailbox;
use lettre::message::header::ContentType;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

struct Mailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
    base_url: String,
}

static MAILER: LazyLock<Option<Mailer>> = LazyLock::new(|| {
    let url = std::env::var("SMTP_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())?;
    let transport = match AsyncSmtpTransport::<Tokio1Executor>::from_url(url.trim()) {
        Ok(b) => b.build(),
        Err(e) => {
            tracing::error!(error = %e, "SMTP_URL is invalid; email disabled");
            return None;
        }
    };
    let from_raw = std::env::var("SMTP_FROM").unwrap_or_else(|_| "noreply@localhost".into());
    let from = match from_raw.parse::<Mailbox>() {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "SMTP_FROM is not a valid address; email disabled");
            return None;
        }
    };
    let base_url = std::env::var("PUBLIC_BASE_URL")
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_string();
    Some(Mailer {
        transport,
        from,
        base_url,
    })
});

pub fn enabled() -> bool {
    MAILER.is_some()
}

/// Plain-text body: the message, then an absolute link to the in-app page.
pub fn render_body(body: &str, base_url: &str, path: Option<&str>) -> String {
    let mut text = body.trim().to_string();
    if let Some(path) = path.filter(|p| p.starts_with('/')) {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(&format!("{base_url}{path}"));
    }
    text.push_str(
        "\n\n--\nYou receive this because email notifications are on in Settings → Notifications.",
    );
    text
}

/// Send one notification email. Best-effort, like push.
pub async fn send(to: &str, subject: &str, body: &str, path: Option<&str>) {
    let Some(m) = MAILER.as_ref() else {
        return;
    };
    let Ok(to) = to.parse::<Mailbox>() else {
        tracing::warn!("recipient address is not valid; email skipped");
        return;
    };
    // Header injection is impossible through lettre's builder, but keep subjects
    // on one line and short.
    let subject: String = subject
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(150)
        .collect();
    let msg = Message::builder()
        .from(m.from.clone())
        .to(to)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(render_body(body, &m.base_url, path));
    let msg = match msg {
        Ok(msg) => msg,
        Err(e) => {
            tracing::warn!(error = %e, "building email failed");
            return;
        }
    };
    if let Err(e) = m.transport.send(msg).await {
        tracing::warn!(error = %e, "sending email failed");
    }
}

#[cfg(test)]
mod tests {
    use super::render_body;

    #[test]
    fn body_links_to_the_app_page() {
        let b = render_body(
            "Oil change due",
            "https://cars.example",
            Some("/app/cars/1"),
        );
        assert!(b.starts_with("Oil change due\n\nhttps://cars.example/app/cars/1"));
    }

    #[test]
    fn only_in_app_paths_are_linked() {
        let b = render_body("x", "https://cars.example", Some("https://evil.example"));
        assert!(!b.contains("evil"));
    }
}
