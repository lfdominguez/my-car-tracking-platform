//! Browser security headers applied to all responses.

use std::path::Path;

use axum::http::{HeaderName, HeaderValue, header};
use base64::Engine;
use sha2::{Digest, Sha256};
use tower_http::set_header::SetResponseHeaderLayer;

const PERMISSIONS_POLICY: &str = "accelerometer=(), camera=(), geolocation=(), gyroscope=(), magnetometer=(), microphone=(), payment=(), usb=()";

/// CSP hash token for an inline script body (`'sha256-...'`).
pub fn script_body_csp_hash(body: &str) -> String {
    let digest = Sha256::digest(body.as_bytes());
    format!(
        "sha256-{}",
        base64::engine::general_purpose::STANDARD.encode(digest)
    )
}

/// Collect CSP hashes for every inline `<script>` body in `html`.
/// External scripts (`src=...`) are ignored — they are covered by `script-src 'self'`.
pub fn inline_script_csp_hashes(html: &str) -> Vec<String> {
    let mut hashes = Vec::new();
    let lower = html.to_ascii_lowercase();
    let bytes = html.as_bytes();
    let lower_bytes = lower.as_bytes();
    let mut i = 0;
    while i < lower_bytes.len() {
        // Find next <script
        let Some(rel) = find_subslice(&lower_bytes[i..], b"<script") else {
            break;
        };
        let start_tag = i + rel;
        let after_name = start_tag + b"<script".len();
        // Find end of opening tag >
        let Some(gt_rel) = find_subslice(&lower_bytes[after_name..], b">") else {
            break;
        };
        let open_end = after_name + gt_rel;
        let open_tag = &lower_bytes[start_tag..=open_end];
        // Skip external scripts
        if find_subslice(open_tag, b"src=").is_some() || find_subslice(open_tag, b"src =").is_some()
        {
            i = open_end + 1;
            continue;
        }
        let body_start = open_end + 1;
        let Some(close_rel) = find_subslice(&lower_bytes[body_start..], b"</script") else {
            break;
        };
        let body_end = body_start + close_rel;
        // Body must be sliced from original HTML (case-preserving).
        if let Ok(body) = std::str::from_utf8(&bytes[body_start..body_end]) {
            hashes.push(script_body_csp_hash(body));
        }
        i = body_end + b"</script".len();
    }
    hashes
}

/// Read `index.html` under `dist` and return inline script CSP hashes (empty if missing).
pub fn inline_script_csp_hashes_from_dist(dist: &Path) -> Vec<String> {
    let index = dist.join("index.html");
    match std::fs::read_to_string(&index) {
        Ok(html) => {
            let hashes = inline_script_csp_hashes(&html);
            if hashes.is_empty() {
                tracing::debug!(path = %index.display(), "no inline scripts in SPA index.html");
            } else {
                tracing::info!(
                    path = %index.display(),
                    count = hashes.len(),
                    "CSP allowing Trunk inline bootstrap via sha256 hashes"
                );
            }
            hashes
        }
        Err(e) => {
            tracing::debug!(
                path = %index.display(),
                error = %e,
                "SPA index.html not readable; CSP without inline script hashes"
            );
            Vec::new()
        }
    }
}

/// Cloudflare Web Analytics beacon host (injected by CF when enabled on the zone).
pub const CLOUDFLARE_INSIGHTS_SCRIPT_HOST: &str = "https://static.cloudflareinsights.com";

/// Build full CSP string. `script_hashes` are bare `sha256-...` tokens (no quotes).
///
/// When `allow_cloudflare_analytics` is true, `script-src` also allows the Cloudflare
/// Web Analytics beacon host. Does **not** add `'unsafe-eval'` or `'unsafe-inline'`.
/// Residual beacon `eval` console noise is expected; the external script can load.
/// Hosts the basemap style (OpenFreeMap Liberty) loads tiles, glyphs and sprites from.
pub const MAP_TILE_HOSTS: &[&str] = &["https://tiles.openfreemap.org"];
/// Where the Cloudflare Web Analytics beacon reports to.
pub const CLOUDFLARE_INSIGHTS_REPORT_HOST: &str = "https://cloudflareinsights.com";

/// Parse `CSP_EXTRA_HOSTS` (comma-separated). Only `https://host[:port]` origins are
/// accepted, so the variable cannot widen the policy back to a scheme wildcard.
pub fn parse_extra_hosts(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|h| {
            h.strip_prefix("https://").is_some_and(|rest| {
                !rest.is_empty()
                    && rest
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'))
            })
        })
        .map(str::to_string)
        .collect()
}

pub fn build_csp(
    script_hashes: &[String],
    allow_cloudflare_analytics: bool,
    extra_hosts: &[String],
) -> String {
    let mut script_src = String::from("script-src 'self' 'wasm-unsafe-eval'");
    for h in script_hashes {
        let token = h.trim();
        if token.is_empty() {
            continue;
        }
        // Accept either sha256-... or 'sha256-...'
        if token.starts_with('\'') {
            script_src.push(' ');
            script_src.push_str(token);
        } else {
            script_src.push_str(" '");
            script_src.push_str(token);
            script_src.push('\'');
        }
    }
    if allow_cloudflare_analytics {
        script_src.push(' ');
        script_src.push_str(CLOUDFLARE_INSIGHTS_SCRIPT_HOST);
    }
    // CSP aligned with self-hosted SPA assets under `/` and `/vendor/*`.
    // Trunk injects an inline module bootstrap into index.html — allow via hash
    // (not 'unsafe-inline') so rebuilds work after server restart rescans dist.
    //
    // connect-src and img-src name the hosts the app really talks to instead of
    // `https:`: with a wildcard, an injected script or an attacker-chosen markdown
    // image in an AI answer could send the viewer's data to any server.
    // style-src keeps 'unsafe-inline' because Leptos views and MapLibre set inline
    // style attributes.
    let mut remote: Vec<&str> = MAP_TILE_HOSTS.to_vec();
    if allow_cloudflare_analytics {
        remote.push(CLOUDFLARE_INSIGHTS_REPORT_HOST);
    }
    remote.extend(extra_hosts.iter().map(String::as_str));
    let remote = remote.join(" ");
    format!(
        "default-src 'self'; \
{script_src}; \
style-src 'self' 'unsafe-inline'; \
img-src 'self' data: blob: {remote}; \
font-src 'self' data:; \
connect-src 'self' {remote}; \
worker-src 'self' blob:; \
child-src 'self' blob:; \
frame-ancestors 'none'; \
base-uri 'self'; \
form-action 'self'; \
object-src 'none'"
    )
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

type HeaderLayer = SetResponseHeaderLayer<HeaderValue>;

/// The security-header layers applied to every response. The last is optional
/// because HSTS is only set when the deployment terminates TLS.
type SecurityHeaderLayers = (
    HeaderLayer,
    HeaderLayer,
    HeaderLayer,
    HeaderLayer,
    HeaderLayer,
    Option<HeaderLayer>,
);

pub fn security_headers_layer(
    enable_hsts: bool,
    script_hashes: &[String],
    allow_cloudflare_analytics: bool,
    extra_hosts: &[String],
) -> SecurityHeaderLayers {
    let nosniff = SetResponseHeaderLayer::overriding(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    let referrer = SetResponseHeaderLayer::overriding(
        header::REFERRER_POLICY,
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    let frame = SetResponseHeaderLayer::overriding(
        header::X_FRAME_OPTIONS,
        HeaderValue::from_static("DENY"),
    );
    let csp_value = build_csp(script_hashes, allow_cloudflare_analytics, extra_hosts);
    let csp_header = HeaderValue::from_str(&csp_value).unwrap_or_else(|_| {
        tracing::error!("invalid CSP header bytes; falling back to strict default");
        HeaderValue::from_static("default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; object-src 'none'; frame-ancestors 'none'")
    });
    let csp = SetResponseHeaderLayer::overriding(header::CONTENT_SECURITY_POLICY, csp_header);
    let permissions = SetResponseHeaderLayer::overriding(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static(PERMISSIONS_POLICY),
    );
    let hsts = if enable_hsts {
        Some(SetResponseHeaderLayer::overriding(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000; includeSubDomains"),
        ))
    } else {
        None
    };
    (nosniff, referrer, frame, csp, permissions, hsts)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trunk injects a module bootstrap into dist/index.html; browsers hash the
    /// exact bytes between the script tags (CSP level 2).
    const TRUNK_BOOTSTRAP: &str = r#"
import init, * as bindings from '/web-d3874c0b7aa4bba4.js';
const wasm = await init({ module_or_path: '/web-d3874c0b7aa4bba4_bg.wasm' });


window.wasmBindings = bindings;


dispatchEvent(new CustomEvent("TrunkApplicationStarted", {detail: {wasm}}));

"#;

    /// Hash reported by Firefox/Chrome for the current Trunk dist bootstrap.
    const TRUNK_BOOTSTRAP_HASH: &str = "sha256-96qy5hJFZcemgMC0EFqDAWIiU4rIp75GcqVfQKTI/Rw=";

    #[test]
    fn csp_hash_for_script_body_matches_browser() {
        let hash = script_body_csp_hash(TRUNK_BOOTSTRAP);
        assert_eq!(hash, TRUNK_BOOTSTRAP_HASH);
    }

    #[test]
    fn extracts_inline_module_script_hashes_from_html() {
        let html = format!(
            r#"<!DOCTYPE html><html><head>
<script type="module">{body}</script>
<script src="/vendor/maplibre-gl.js"></script>
</head><body></body></html>"#,
            body = TRUNK_BOOTSTRAP
        );
        let hashes = inline_script_csp_hashes(&html);
        assert_eq!(hashes, vec![TRUNK_BOOTSTRAP_HASH.to_string()]);
    }

    #[test]
    fn build_csp_allows_trunk_inline_via_hash_not_unsafe_inline() {
        let csp = build_csp(&[TRUNK_BOOTSTRAP_HASH.to_string()], false, &[]);
        assert!(csp.contains("script-src 'self' 'wasm-unsafe-eval'"));
        // Prefer quoted hash token form in CSP.
        assert!(
            csp.contains(&format!("'{TRUNK_BOOTSTRAP_HASH}'")),
            "csp={csp}"
        );
        // style-src may keep 'unsafe-inline' for app CSS; script-src must not.
        let script_src = csp
            .split(';')
            .map(str::trim)
            .find(|d| d.starts_with("script-src"))
            .expect("script-src directive");
        assert!(
            !script_src.contains("'unsafe-inline'"),
            "script-src must not allow unsafe-inline: {script_src}"
        );
        assert!(
            !script_src.contains(CLOUDFLARE_INSIGHTS_SCRIPT_HOST),
            "CF analytics host must be off by default: {script_src}"
        );
        assert!(
            !script_src.contains("'unsafe-eval'"),
            "script-src must not allow unsafe-eval: {script_src}"
        );
    }

    #[test]
    fn build_csp_optional_cloudflare_analytics_script_host() {
        let off = build_csp(&[], false, &[]);
        let on = build_csp(&[], true, &[]);
        let off_script = off
            .split(';')
            .map(str::trim)
            .find(|d| d.starts_with("script-src"))
            .expect("script-src");
        let on_script = on
            .split(';')
            .map(str::trim)
            .find(|d| d.starts_with("script-src"))
            .expect("script-src");
        assert!(!off_script.contains(CLOUDFLARE_INSIGHTS_SCRIPT_HOST));
        assert!(
            on_script.contains(CLOUDFLARE_INSIGHTS_SCRIPT_HOST),
            "on script-src={on_script}"
        );
        assert!(
            !on_script.contains("'unsafe-eval'"),
            "CF mode must not add unsafe-eval: {on_script}"
        );
        assert!(
            !on_script.contains("'unsafe-inline'"),
            "CF mode must not add unsafe-inline: {on_script}"
        );
    }

    #[test]
    fn reads_hashes_from_dist_index_when_present() {
        // Prefer real dist if built; otherwise synthetic file is enough for unit shape.
        let html = format!(r#"<!DOCTYPE html><script type="module">{TRUNK_BOOTSTRAP}</script>"#);
        let hashes = inline_script_csp_hashes(&html);
        assert_eq!(hashes.len(), 1);
        assert_eq!(hashes[0], TRUNK_BOOTSTRAP_HASH);
    }

    #[test]
    fn csp_names_hosts_instead_of_https_wildcard() {
        let csp = build_csp(
            &[],
            false,
            &parse_extra_hosts("https://tiles.example.com, http://x, https:, javascript:x"),
        );
        for directive in ["connect-src", "img-src"] {
            let d = csp
                .split(';')
                .map(str::trim)
                .find(|d| d.starts_with(directive))
                .unwrap();
            assert!(!d.split_whitespace().any(|t| t == "https:"), "{d}");
            assert!(d.contains("https://tiles.openfreemap.org"), "{d}");
            assert!(d.contains("https://tiles.example.com"), "{d}");
            assert!(!d.contains("http://x"), "{d}");
        }
    }
}
