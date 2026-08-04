use leptos::prelude::*;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = r#"
export function renderTripMap(elId, geojson) {
  const el = document.getElementById(elId);
  if (!el || !window.maplibregl) return;
  el.innerHTML = '';
  const map = new maplibregl.Map({
    container: el,
    style: 'https://demotiles.maplibre.org/style.json',
    center: [0, 0],
    zoom: 1
  });
  map.on('load', () => {
    map.addSource('trip', { type: 'geojson', data: geojson });
    map.addLayer({
      id: 'trip-line',
      type: 'line',
      source: 'trip',
      paint: { 'line-color': '#3b82f6', 'line-width': 4 }
    });
    const coords = geojson.coordinates || [];
    if (coords.length > 0) {
      const bounds = coords.reduce((b, c) => b.extend(c), new maplibregl.LngLatBounds(coords[0], coords[0]));
      map.fitBounds(bounds, { padding: 40, maxZoom: 14 });
    }
  });
}
"#)]
extern "C" {
    fn renderTripMap(el_id: &str, geojson: &JsValue);
}

#[component]
pub fn TripMap(geojson: Signal<Option<serde_json::Value>>) -> impl IntoView {
    let id = "trip-map";
    Effect::new(move |_| {
        if let Some(gj) = geojson.get() {
            if let Ok(js) = serde_wasm_bindgen_compat(&gj) {
                renderTripMap(id, &js);
            }
        }
    });
    view! { <div id=id class="map"></div> }
}

fn serde_wasm_bindgen_compat(v: &serde_json::Value) -> Result<JsValue, String> {
    let s = serde_json::to_string(v).map_err(|e| e.to_string())?;
    js_sys::JSON::parse(&s).map_err(|e| format!("{e:?}"))
}
