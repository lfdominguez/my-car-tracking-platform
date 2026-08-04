use leptos::prelude::*;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = r#"
export function renderQr(elId, text) {
  const el = document.getElementById(elId);
  if (!el || !window.QRCode) return;
  el.innerHTML = '';
  QRCode.toCanvas(document.createElement('canvas'), text, { width: 256, margin: 1 }, function (err, canvas) {
    if (err) { el.textContent = String(err); return; }
    el.appendChild(canvas);
  });
}
"#)]
extern "C" {
    fn renderQr(el_id: &str, text: &str);
}

#[component]
pub fn QrCode(payload: Signal<Option<String>>) -> impl IntoView {
    let id = "qr-code";
    Effect::new(move |_| {
        if let Some(text) = payload.get() {
            renderQr(id, &text);
        }
    });
    view! { <div id=id class="qr-box"></div> }
}
