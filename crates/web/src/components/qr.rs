use leptos::prelude::*;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = r#"
/** UI text published by the app (src/i18n.rs) for the current language. */
function tt(key, fallback) {
  const table = window.__ctpI18n;
  return (table && table[key]) || fallback;
}

/**
 * Load a self-hosted vendor script on first use instead of blocking <head>.
 * Same-origin /vendor URLs keep CSP `script-src 'self'` satisfied. The promise is
 * shared on `window`, so every snippet that needs the library waits on one fetch.
 */
function loadVendorScript(src, globalName) {
  if (window[globalName]) return Promise.resolve();
  const loads = (window.__ctpVendorLoads = window.__ctpVendorLoads || {});
  if (!loads[src]) {
    loads[src] = new Promise((resolve, reject) => {
      const s = document.createElement('script');
      s.src = src;
      s.async = true;
      s.onload = () => (window[globalName] ? resolve() : reject(new Error(`${src} loaded without ${globalName}`)));
      s.onerror = () => {
        delete loads[src];
        reject(new Error(`failed to load ${src}`));
      };
      document.head.appendChild(s);
    });
  }
  return loads[src];
}

export function renderQr(elId, text) {
  const el = document.getElementById(elId);
  if (!el) return 'missing-el';
  if (typeof window.QRCode === 'undefined' || typeof window.QRCode.toCanvas !== 'function') {
    return 'missing-lib';
  }
  el.innerHTML = '';
  // qrcode.toCanvas(canvas, text, opts, cb) draws onto the given canvas and
  // only passes `error` to the callback (not the canvas). Append first, then draw.
  const canvas = document.createElement('canvas');
  el.appendChild(canvas);
  try {
    window.QRCode.toCanvas(
      canvas,
      text,
      { width: 512, margin: 2, errorCorrectionLevel: 'M', color: { dark: '#111827', light: '#ffffff' } },
      function (err) {
        if (err) {
          el.textContent = String(err);
        }
      }
    );
  } catch (e) {
    el.textContent = String(e);
    return 'error';
  }
  return 'ok';
}

export function clearQr(elId) {
  const el = document.getElementById(elId);
  if (el) el.innerHTML = '';
}

export function scheduleRenderQr(elId, text) {
  // Retry while host mounts and/or vendor script finishes loading.
  // Also re-check window.QRCode — a relative script src on /cars/:id can 404
  // as SPA HTML; absolute /qrcode.min.js is required in index.html.
  let attempts = 0;
  // index.html loads the library with `defer`; if that tag is missing or failed,
  // fetch it here so the retries below have something to wait for.
  loadVendorScript('/qrcode.min.js', 'QRCode').catch(() => {});
  function tick() {
    attempts += 1;
    const status = renderQr(elId, text);
    if (status === 'ok' || status === 'error') return;
    if (attempts >= 60) {
      const el = document.getElementById(elId);
      if (!el) return;
      if (status === 'missing-lib') {
        el.textContent = tt('js.qr.no_lib', 'QR library not loaded');
      } else {
        el.textContent = tt('js.qr.not_ready', 'QR container not ready');
      }
      return;
    }
    setTimeout(tick, 50);
  }
  tick();
}
"#)]
extern "C" {
    fn scheduleRenderQr(el_id: &str, text: &str);
    fn clearQr(el_id: &str);
}

/// Renders a QR code for the given payload JSON (Android settings bootstrap).
#[component]
pub fn QrCode(#[prop(into)] payload: Signal<Option<String>>) -> impl IntoView {
    // Unique id so multiple instances / remounts do not clash.
    let id = {
        let n = (js_sys::Math::random() * 1_000_000.0).floor() as u32;
        format!("qr-code-{n}")
    };
    let el_id = id.clone();
    let el_id_clear = id.clone();

    Effect::new(move |_| match payload.get() {
        Some(text) => scheduleRenderQr(&el_id, &text),
        None => clearQr(&el_id_clear),
    });

    let hidden = Signal::derive(move || payload.get().is_none());

    view! {
        <div
            id=id
            class="qr-box"
            aria-label=tr!("qr.label")
            // Keep the node mounted so canvas draw is reliable; hide until we have payload.
            style=move || {
                if hidden.get() {
                    "display:none"
                } else {
                    "display:flex"
                }
            }
        ></div>
    }
}
