use leptos::prelude::*;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = r#"
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
      { width: 256, margin: 2, color: { dark: '#111827', light: '#ffffff' } },
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
  let attempts = 0;
  function tick() {
    attempts += 1;
    const status = renderQr(elId, text);
    if (status === 'ok' || status === 'error') return;
    if (attempts >= 40) {
      const el = document.getElementById(elId);
      if (!el) return;
      if (status === 'missing-lib') {
        el.textContent = 'QR library not loaded';
      } else {
        el.textContent = 'QR container not ready';
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
            aria-label="Device provisioning QR code"
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
