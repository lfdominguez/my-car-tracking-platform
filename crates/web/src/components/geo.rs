//! Small MapLibre maps beyond the trip cockpit: live car markers (#108), many
//! trip lines with a heatmap toggle (#120) and two-route comparison (#122).
//!
//! MapLibre is loaded on first use, exactly like the trip map. Each map is keyed
//! by its element id and torn down when its component unmounts. Clicking a line
//! dispatches a `geo-map-select` event on `window` (`detail: { el, id }`) so the
//! page can route to the trip without handing a Rust closure to JS.

use leptos::prelude::*;
use serde::Serialize;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = r#"
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
function loadVendorCss(href) {
  const loads = (window.__ctpVendorLoads = window.__ctpVendorLoads || {});
  if (!loads[href]) {
    loads[href] = new Promise((resolve) => {
      const l = document.createElement('link');
      l.rel = 'stylesheet';
      l.href = href;
      l.onload = l.onerror = () => resolve();
      document.head.appendChild(l);
    });
  }
  return loads[href];
}
function loadMapLibre() {
  return Promise.all([
    loadVendorCss('/vendor/maplibre-gl.css'),
    loadVendorScript('/vendor/maplibre-gl.js', 'maplibregl'),
  ]);
}

const GEO_STYLE = 'https://tiles.openfreemap.org/styles/liberty';
const __geoMaps = new Map();
const __geoPending = new Map();

function cssVar(name, fallback) {
  try {
    const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
    return v || fallback;
  } catch (_) {
    return fallback;
  }
}

/** Run `fn(entry)` once MapLibre and the map's style are ready. Latest call wins. */
function withGeoMap(elId, fn) {
  if (!window.maplibregl) {
    __geoPending.set(elId, fn);
    loadMapLibre()
      .then(() => {
        const f = __geoPending.get(elId);
        __geoPending.delete(elId);
        if (f) withGeoMap(elId, f);
      })
      .catch((err) => console.error('MapLibre failed to load', err));
    return;
  }
  const el = document.getElementById(elId);
  if (!el) return;
  let entry = __geoMaps.get(elId);
  if (entry && entry.container !== el) {
    disposeGeoMap(elId);
    entry = null;
  }
  if (!entry) {
    el.innerHTML = '';
    const map = new maplibregl.Map({
      container: el,
      style: GEO_STYLE,
      center: [0, 20],
      zoom: 1.5,
      attributionControl: true,
      antialias: false,
      failIfMajorPerformanceCaveat: false,
    });
    map.addControl(new maplibregl.NavigationControl({ showCompass: true }), 'top-right');
    // Missing sprite images must not spam the console or break layers.
    map.on('styleimagemissing', (e) => {
      try {
        if (!map.hasImage(e.id)) map.addImage(e.id, { width: 1, height: 1, data: new Uint8Array(4) });
      } catch (_) {}
    });
    entry = { map, container: el, markers: new Map(), ready: false, queue: null, fitKey: null };
    __geoMaps.set(elId, entry);
    map.once('load', () => {
      entry.ready = true;
      const q = entry.queue;
      entry.queue = null;
      if (q) q(entry);
    });
  }
  if (entry.ready) fn(entry);
  else entry.queue = fn;
}

function fitTo(map, coords, maxZoom) {
  if (!coords.length) return;
  const b = new maplibregl.LngLatBounds(coords[0], coords[0]);
  for (const c of coords) b.extend(c);
  map.fitBounds(b, { padding: 56, maxZoom: maxZoom || 15, duration: 0 });
}

function escapeHtml(s) {
  return String(s == null ? '' : s)
    .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

/** Live markers: one per car, rotated by heading; driving cars pulse. */
export function geoRenderLive(elId, json) {
  let cars = [];
  try { cars = JSON.parse(json) || []; } catch (_) { return; }
  withGeoMap(elId, (entry) => {
    const seen = new Set();
    for (const c of cars) {
      if (!Number.isFinite(c.lat) || !Number.isFinite(c.lon)) continue;
      seen.add(c.id);
      let m = entry.markers.get(c.id);
      if (!m) {
        const el = document.createElement('div');
        el.className = 'live-marker';
        el.innerHTML = '<span class="live-marker-dot"><span class="live-marker-arrow"></span></span><span class="live-marker-label"></span>';
        m = new maplibregl.Marker({ element: el, anchor: 'center' }).setLngLat([c.lon, c.lat]).addTo(entry.map);
        entry.markers.set(c.id, m);
      }
      m.setLngLat([c.lon, c.lat]);
      const el = m.getElement();
      el.classList.toggle('is-driving', !!c.driving);
      el.classList.toggle('has-heading', Number.isFinite(c.heading));
      const arrow = el.querySelector('.live-marker-arrow');
      if (arrow) arrow.style.transform = Number.isFinite(c.heading) ? `rotate(${c.heading}deg)` : '';
      const label = el.querySelector('.live-marker-label');
      if (label) label.innerHTML = escapeHtml(c.label);
      el.setAttribute('title', c.title || c.label || '');
    }
    for (const [id, m] of entry.markers.entries()) {
      if (!seen.has(id)) {
        m.remove();
        entry.markers.delete(id);
      }
    }
    // Frame the fleet once per set of cars, not on every position update.
    const key = [...seen].sort().join(',');
    if (key && key !== entry.fitKey) {
      entry.fitKey = key;
      fitTo(entry.map, cars.filter((c) => seen.has(c.id)).map((c) => [c.lon, c.lat]), 14);
    }
  });
}

/**
 * Many lines at once. `json` = { features: [{ id, color, coordinates }], mode:
 * 'lines' | 'heat', clickable, fitKey }.
 */
export function geoRenderLines(elId, json) {
  let data;
  try { data = JSON.parse(json) || {}; } catch (_) { return; }
  withGeoMap(elId, (entry) => {
    const map = entry.map;
    const accent = cssVar('--color-accent', '#5a9aff');
    const features = (data.features || []).map((f) => ({
      type: 'Feature',
      properties: { id: f.id, color: f.color || accent },
      geometry: { type: 'LineString', coordinates: f.coordinates || [] },
    }));
    const points = [];
    for (const f of features) {
      for (const c of f.geometry.coordinates) {
        points.push({ type: 'Feature', properties: {}, geometry: { type: 'Point', coordinates: c } });
      }
    }
    const lineFc = { type: 'FeatureCollection', features };
    const pointFc = { type: 'FeatureCollection', features: points };
    if (!map.getSource('geo-lines')) {
      map.addSource('geo-lines', { type: 'geojson', data: lineFc });
      map.addSource('geo-points', { type: 'geojson', data: pointFc });
      map.addLayer({
        id: 'geo-heat',
        type: 'heatmap',
        source: 'geo-points',
        layout: { visibility: 'none' },
        paint: {
          'heatmap-radius': ['interpolate', ['linear'], ['zoom'], 4, 4, 10, 10, 15, 22],
          'heatmap-intensity': ['interpolate', ['linear'], ['zoom'], 4, 0.6, 15, 1.6],
          'heatmap-opacity': 0.85,
          'heatmap-color': [
            'interpolate', ['linear'], ['heatmap-density'],
            0, 'rgba(0,0,0,0)',
            0.15, '#2b6cff',
            0.4, '#37d9e8',
            0.65, '#ffb545',
            0.9, '#ff6b63',
          ],
        },
      });
      map.addLayer({
        id: 'geo-lines-casing',
        type: 'line',
        source: 'geo-lines',
        layout: { 'line-cap': 'round', 'line-join': 'round' },
        paint: { 'line-color': 'rgba(255,255,255,0.85)', 'line-width': 6 },
      });
      map.addLayer({
        id: 'geo-lines',
        type: 'line',
        source: 'geo-lines',
        layout: { 'line-cap': 'round', 'line-join': 'round' },
        paint: { 'line-color': ['get', 'color'], 'line-width': 3.5, 'line-opacity': 0.8 },
      });
      map.addLayer({
        id: 'geo-lines-hit',
        type: 'line',
        source: 'geo-lines',
        paint: { 'line-color': '#000', 'line-width': 14, 'line-opacity': 0.01 },
      });
      map.on('mouseenter', 'geo-lines-hit', () => {
        if (entry.clickable) map.getCanvas().style.cursor = 'pointer';
      });
      map.on('mouseleave', 'geo-lines-hit', () => { map.getCanvas().style.cursor = ''; });
      map.on('click', 'geo-lines-hit', (e) => {
        if (!entry.clickable) return;
        const f = e.features && e.features[0];
        const id = f && f.properties && f.properties.id;
        if (id) window.dispatchEvent(new CustomEvent('geo-map-select', { detail: { el: elId, id: String(id) } }));
      });
    } else {
      map.getSource('geo-lines').setData(lineFc);
      map.getSource('geo-points').setData(pointFc);
    }
    entry.clickable = !!data.clickable;
    const heat = data.mode === 'heat';
    map.setLayoutProperty('geo-heat', 'visibility', heat ? 'visible' : 'none');
    for (const id of ['geo-lines-casing', 'geo-lines', 'geo-lines-hit']) {
      map.setLayoutProperty(id, 'visibility', heat ? 'none' : 'visible');
    }
    const key = String(data.fitKey || '');
    if (key !== entry.fitKey) {
      entry.fitKey = key;
      fitTo(map, points.map((p) => p.geometry.coordinates), 15);
    }
  });
}

/** Move (or hide, with non-finite values) a single highlight marker. */
export function geoSetCursor(elId, lon, lat) {
  const entry = __geoMaps.get(elId);
  if (!entry || !entry.ready) return;
  if (!Number.isFinite(lon) || !Number.isFinite(lat)) {
    if (entry.cursor) { entry.cursor.remove(); entry.cursor = null; }
    return;
  }
  if (!entry.cursor) {
    const el = document.createElement('div');
    el.className = 'geo-cursor';
    entry.cursor = new maplibregl.Marker({ element: el }).setLngLat([lon, lat]).addTo(entry.map);
  } else {
    entry.cursor.setLngLat([lon, lat]);
  }
}

export function disposeGeoMap(elId) {
  __geoPending.delete(elId);
  const entry = __geoMaps.get(elId);
  if (!entry) return;
  try { for (const m of entry.markers.values()) m.remove(); } catch (_) {}
  try { entry.cursor && entry.cursor.remove(); } catch (_) {}
  try { entry.map.remove(); } catch (_) {}
  __geoMaps.delete(elId);
}
"#)]
extern "C" {
    fn geoRenderLive(el_id: &str, json: &str);
    fn disposeGeoMap(el_id: &str);
}

/// One car on the live map.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LiveMarker {
    pub id: String,
    pub lat: f64,
    pub lon: f64,
    /// Degrees clockwise from north; `None` when parked or unknown.
    pub heading: Option<f64>,
    pub label: String,
    pub title: String,
    pub driving: bool,
}

/// Live car markers (#108).
#[component]
pub fn LiveMap(
    #[prop(into)] markers: Signal<Vec<LiveMarker>>,
    #[prop(default = "live-map")] id: &'static str,
) -> impl IntoView {
    on_cleanup(move || disposeGeoMap(id));
    Effect::new(move |_| {
        let Some(list) = markers.try_get() else {
            return;
        };
        if let Ok(json) = serde_json::to_string(&list) {
            geoRenderLive(id, &json);
        }
    });
    view! { <div id=id class="map live-map"></div> }
}
