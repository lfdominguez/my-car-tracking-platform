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

/**
 * Areas (places): `json` = { areas: [{ id, ring: [[lon, lat], ...], draft, label }],
 * vertices: [[lon, lat], ...], fitKey }. Map clicks dispatch `geo-map-click` with
 * `{ el, lon, lat }`.
 */
export function geoRenderAreas(elId, json) {
  let data;
  try { data = JSON.parse(json) || {}; } catch (_) { return; }
  withGeoMap(elId, (entry) => {
    const map = entry.map;
    const accent = cssVar('--color-accent', '#5a9aff');
    const warn = cssVar('--color-warning', '#ffb545');
    const areaFc = {
      type: 'FeatureCollection',
      features: (data.areas || []).filter((a) => (a.ring || []).length >= 3).map((a) => ({
        type: 'Feature',
        properties: { id: a.id, draft: !!a.draft, label: a.label || '' },
        geometry: { type: 'Polygon', coordinates: [[...a.ring, a.ring[0]]] },
      })),
    };
    const vertexFc = {
      type: 'FeatureCollection',
      features: (data.vertices || []).map((c) => ({ type: 'Feature', properties: {}, geometry: { type: 'Point', coordinates: c } })),
    };
    if (!map.getSource('geo-areas')) {
      map.addSource('geo-areas', { type: 'geojson', data: areaFc });
      map.addSource('geo-vertices', { type: 'geojson', data: vertexFc });
      map.addLayer({
        id: 'geo-areas-fill', type: 'fill', source: 'geo-areas',
        paint: { 'fill-color': ['case', ['get', 'draft'], warn, accent], 'fill-opacity': 0.18 },
      });
      map.addLayer({
        id: 'geo-areas-line', type: 'line', source: 'geo-areas',
        paint: {
          'line-color': ['case', ['get', 'draft'], warn, accent],
          'line-width': ['case', ['get', 'draft'], 3, 2],
        },
      });
      map.addLayer({
        id: 'geo-areas-label', type: 'symbol', source: 'geo-areas',
        layout: { 'text-field': ['get', 'label'], 'text-size': 12, 'text-allow-overlap': false },
        paint: { 'text-color': '#1b2330', 'text-halo-color': 'rgba(255,255,255,0.95)', 'text-halo-width': 1.6 },
      });
      map.addLayer({
        id: 'geo-vertices', type: 'circle', source: 'geo-vertices',
        paint: { 'circle-radius': 5, 'circle-color': warn, 'circle-stroke-color': '#fff', 'circle-stroke-width': 2 },
      });
      map.getCanvas().style.cursor = 'crosshair';
      map.on('click', (e) => {
        window.dispatchEvent(new CustomEvent('geo-map-click', {
          detail: { el: elId, lon: e.lngLat.lng, lat: e.lngLat.lat },
        }));
      });
    } else {
      map.getSource('geo-areas').setData(areaFc);
      map.getSource('geo-vertices').setData(vertexFc);
    }
    const key = String(data.fitKey || '');
    if (key && key !== entry.fitKey) {
      entry.fitKey = key;
      const pts = [];
      for (const f of areaFc.features) for (const c of f.geometry.coordinates[0]) pts.push(c);
      fitTo(map, pts, 15);
    }
  });
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
    fn geoRenderLines(el_id: &str, json: &str);
    fn geoRenderAreas(el_id: &str, json: &str);
    fn geoSetCursor(el_id: &str, lon: f64, lat: f64);
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

/// One line on a [`LinesMap`].
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MapLine {
    pub id: String,
    /// CSS color; the accent when `None`.
    pub color: Option<String>,
    /// `[lon, lat]` pairs.
    pub coordinates: Vec<[f64; 2]>,
}

/// What a [`LinesMap`] draws.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct LinesData {
    pub features: Vec<MapLine>,
    /// `lines` or `heat` (vertices as a heatmap).
    pub mode: &'static str,
    /// Emit `geo-map-select` on click.
    pub clickable: bool,
    /// The map re-frames itself whenever this changes.
    #[serde(rename = "fitKey")]
    pub fit_key: String,
}

/// Many trip lines on one map, with an optional heatmap mode (#120, #122).
#[component]
pub fn LinesMap(
    #[prop(into)] data: Signal<LinesData>,
    id: &'static str,
    /// Optional `[lon, lat]` highlight marker.
    #[prop(optional, into)]
    cursor: Option<Signal<Option<[f64; 2]>>>,
) -> impl IntoView {
    on_cleanup(move || disposeGeoMap(id));
    Effect::new(move |_| {
        let Some(d) = data.try_get() else {
            return;
        };
        if let Ok(json) = serde_json::to_string(&d) {
            geoRenderLines(id, &json);
        }
    });
    if let Some(cursor) = cursor {
        Effect::new(move |_| {
            let at = cursor.try_get().flatten();
            let (lon, lat) = at.map(|c| (c[0], c[1])).unwrap_or((f64::NAN, f64::NAN));
            geoSetCursor(id, lon, lat);
        });
    }
    view! { <div id=id class="map lines-map"></div> }
}

/// One area on an [`AreasMap`].
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MapArea {
    pub id: String,
    /// Closed ring without the repeated first vertex, `[lon, lat]`.
    pub ring: Vec<[f64; 2]>,
    /// The shape being drawn (highlighted).
    pub draft: bool,
    pub label: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct AreasData {
    pub areas: Vec<MapArea>,
    /// Polygon vertices placed so far.
    pub vertices: Vec<[f64; 2]>,
    #[serde(rename = "fitKey")]
    pub fit_key: String,
}

/// Places / geofences (#111). Clicks arrive as `geo-map-click` window events.
#[component]
pub fn AreasMap(#[prop(into)] data: Signal<AreasData>, id: &'static str) -> impl IntoView {
    on_cleanup(move || disposeGeoMap(id));
    Effect::new(move |_| {
        let Some(d) = data.try_get() else {
            return;
        };
        if let Ok(json) = serde_json::to_string(&d) {
            geoRenderAreas(id, &json);
        }
    });
    view! { <div id=id class="map areas-map"></div> }
}

/// A circle as a 64-vertex ring (`[lon, lat]`), good enough to draw.
pub fn circle_ring(lat: f64, lon: f64, radius_m: f64) -> Vec<[f64; 2]> {
    const R: f64 = 6_371_000.0;
    let d = radius_m / R;
    let (la, lo) = (lat.to_radians(), lon.to_radians());
    (0..64)
        .map(|i| {
            let b = (i as f64) * std::f64::consts::TAU / 64.0;
            let la2 = (la.sin() * d.cos() + la.cos() * d.sin() * b.cos()).asin();
            let lo2 = lo + (b.sin() * d.sin() * la.cos()).atan2(d.cos() - la.sin() * la2.sin());
            [lo2.to_degrees(), la2.to_degrees()]
        })
        .collect()
}

/// `[lon, lat]` pairs from a GeoJSON LineString value.
pub fn line_coordinates(geometry: &serde_json::Value) -> Vec<[f64; 2]> {
    geometry
        .get("coordinates")
        .and_then(|c| c.as_array())
        .map(|pts| {
            pts.iter()
                .filter_map(|p| {
                    let p = p.as_array()?;
                    Some([p.first()?.as_f64()?, p.get(1)?.as_f64()?])
                })
                .collect()
        })
        .unwrap_or_default()
}
