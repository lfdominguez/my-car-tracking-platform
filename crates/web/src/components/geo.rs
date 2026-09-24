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
 * Areas (places): `json` = { areas: [{ id, ring: [[lon, lat], ...], draft, label,
 * label_at: [lon, lat] | null }], vertices: [[lon, lat], ...], fitKey, focusKey }.
 * Map clicks dispatch `geo-map-click` with `{ el, lon, lat }`.
 *
 * Labels live in their own point source. On a polygon source a symbol layer is
 * placed once per tile piece, and when its glyphs fail to load MapLibre drops
 * the whole GeoJSON tile: every named place vanished and the draft circle broke
 * into stray arcs and tile-edge chords. The font stack must be one OpenFreeMap
 * actually serves (a single Noto face), like the trip map's labels.
 */
export function geoRenderAreas(elId, json) {
  let data;
  try { data = JSON.parse(json) || {}; } catch (_) { return; }
  withGeoMap(elId, (entry) => {
    const map = entry.map;
    const accent = cssVar('--color-accent', '#5a9aff');
    const warn = cssVar('--color-warning', '#ffb545');
    const valid = (c) => Array.isArray(c) && Number.isFinite(c[0]) && Number.isFinite(c[1]);
    const closed = (ring) => {
      const r = ring.filter(valid);
      if (r.length < 3) return null;
      const [a, z] = [r[0], r[r.length - 1]];
      if (a[0] !== z[0] || a[1] !== z[1]) r.push(a);
      return r.length >= 4 ? r : null;
    };
    const areas = [];
    for (const a of data.areas || []) {
      const ring = closed(a.ring || []);
      if (ring) areas.push({ a, ring });
    }
    const areaFc = {
      type: 'FeatureCollection',
      features: areas.map(({ a, ring }) => ({
        type: 'Feature',
        properties: { id: a.id, draft: !!a.draft },
        geometry: { type: 'Polygon', coordinates: [ring] },
      })),
    };
    const labelFc = {
      type: 'FeatureCollection',
      features: areas
        .filter(({ a }) => a.label && valid(a.label_at))
        .map(({ a }) => ({
          type: 'Feature',
          properties: { label: a.label, draft: !!a.draft },
          geometry: { type: 'Point', coordinates: a.label_at },
        })),
    };
    const vertexFc = {
      type: 'FeatureCollection',
      features: (data.vertices || []).filter(valid).map((c) => ({ type: 'Feature', properties: {}, geometry: { type: 'Point', coordinates: c } })),
    };
    if (!map.getSource('geo-areas')) {
      map.addSource('geo-areas', { type: 'geojson', data: areaFc });
      map.addSource('geo-area-labels', { type: 'geojson', data: labelFc });
      map.addSource('geo-vertices', { type: 'geojson', data: vertexFc });
      map.addLayer({
        id: 'geo-areas-fill', type: 'fill', source: 'geo-areas',
        paint: { 'fill-color': ['case', ['get', 'draft'], warn, accent], 'fill-opacity': 0.18 },
      });
      map.addLayer({
        id: 'geo-areas-line', type: 'line', source: 'geo-areas',
        layout: { 'line-join': 'round', 'line-cap': 'round' },
        paint: {
          'line-color': ['case', ['get', 'draft'], warn, accent],
          'line-width': ['case', ['get', 'draft'], 3, 2],
        },
      });
      map.addLayer({
        id: 'geo-vertices', type: 'circle', source: 'geo-vertices',
        paint: { 'circle-radius': 5, 'circle-color': warn, 'circle-stroke-color': '#fff', 'circle-stroke-width': 2 },
      });
      map.addLayer({
        id: 'geo-areas-label', type: 'symbol', source: 'geo-area-labels',
        layout: {
          'text-field': ['get', 'label'],
          'text-font': ['Noto Sans Regular'],
          'text-size': 12,
          'text-allow-overlap': false,
        },
        paint: { 'text-color': '#1b2330', 'text-halo-color': 'rgba(255,255,255,0.95)', 'text-halo-width': 1.6 },
      });
    } else {
      map.getSource('geo-areas').setData(areaFc);
      map.getSource('geo-area-labels').setData(labelFc);
      map.getSource('geo-vertices').setData(vertexFc);
    }
    // One click listener per map, whatever happens to the sources.
    if (!entry.areasClick) {
      entry.areasClick = true;
      map.getCanvas().style.cursor = 'crosshair';
      map.on('click', (e) => {
        window.dispatchEvent(new CustomEvent('geo-map-click', {
          detail: { el: elId, lon: e.lngLat.lng, lat: e.lngLat.lat },
        }));
      });
    }
    // Frame every place when the saved set changes, and the draft when a place
    // is picked for editing (`focusKey`); drawing or dragging never re-frames.
    const framePts = (features) => {
      const pts = [];
      for (const f of features) for (const c of f.geometry.coordinates[0]) pts.push(c);
      return pts;
    };
    const key = String(data.fitKey || '');
    if (key && key !== entry.fitKey) {
      entry.fitKey = key;
      fitTo(map, framePts(areaFc.features.filter((f) => !f.properties.draft)), 16);
    }
    const focus = String(data.focusKey || '');
    if (focus !== (entry.focusKey || '')) {
      entry.focusKey = focus;
      if (focus) fitTo(map, framePts(areaFc.features.filter((f) => f.properties.draft)), 16);
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
    /// Outline, `[lon, lat]`. Closed or open: the map closes it when needed.
    pub ring: Vec<[f64; 2]>,
    /// The shape being drawn (highlighted).
    pub draft: bool,
    pub label: String,
    /// Where the label goes (`[lon, lat]`): the center of a circle, the vertex
    /// average of a polygon.
    pub label_at: Option<[f64; 2]>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct AreasData {
    pub areas: Vec<MapArea>,
    /// Polygon vertices placed so far.
    pub vertices: Vec<[f64; 2]>,
    /// The map frames the saved (non-draft) areas whenever this changes.
    #[serde(rename = "fitKey")]
    pub fit_key: String,
    /// When this changes to a non-empty value the map frames the draft
    /// (picking a place to edit); empty never re-frames.
    #[serde(rename = "focusKey")]
    pub focus_key: String,
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

/// Segments of a drawn circle.
pub const CIRCLE_SEGMENTS: usize = 64;

/// A circle of `radius_m` metres around (`lat`, `lon`) as a closed ring of
/// `[lon, lat]` pairs: [`CIRCLE_SEGMENTS`] + 1 points, the last repeating the
/// first. Great-circle destination points, so the east/west spread already
/// accounts for `cos(lat)`.
pub fn circle_ring(lat: f64, lon: f64, radius_m: f64) -> Vec<[f64; 2]> {
    const R: f64 = 6_371_000.0;
    let d = radius_m / R;
    let (la, lo) = (lat.to_radians(), lon.to_radians());
    let mut ring: Vec<[f64; 2]> = (0..CIRCLE_SEGMENTS)
        .map(|i| {
            let b = (i as f64) * std::f64::consts::TAU / CIRCLE_SEGMENTS as f64;
            let la2 = (la.sin() * d.cos() + la.cos() * d.sin() * b.cos()).asin();
            let lo2 = lo + (b.sin() * d.sin() * la.cos()).atan2(d.cos() - la.sin() * la2.sin());
            [lo2.to_degrees(), la2.to_degrees()]
        })
        .collect();
    if let Some(&first) = ring.first() {
        ring.push(first);
    }
    ring
}

/// Label anchor of an outline: the average of its distinct vertices (a closing
/// vertex is not counted twice). `None` for an empty ring.
pub fn ring_label_point(ring: &[[f64; 2]]) -> Option<[f64; 2]> {
    let pts = match ring {
        [first, .., last] if first == last => &ring[..ring.len() - 1],
        _ => ring,
    };
    if pts.is_empty() {
        return None;
    }
    let n = pts.len() as f64;
    let (lon, lat) = pts
        .iter()
        .fold((0.0, 0.0), |(x, y), [lon, lat]| (x + lon, y + lat));
    Some([lon / n, lat / n])
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

#[cfg(test)]
mod tests {
    use super::*;

    const EARTH_R: f64 = 6_371_000.0;

    fn haversine_m([lon1, lat1]: [f64; 2], [lon2, lat2]: [f64; 2]) -> f64 {
        let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
        let dp = p2 - p1;
        let dl = (lon2 - lon1).to_radians();
        let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
        2.0 * EARTH_R * a.sqrt().asin()
    }

    #[test]
    fn circle_ring_is_closed_with_n_plus_one_points() {
        let ring = circle_ring(40.4168, -3.7038, 575.0);
        assert_eq!(ring.len(), CIRCLE_SEGMENTS + 1);
        assert_eq!(ring.first(), ring.last());
        // No other vertex repeats: a proper simple ring.
        for (i, a) in ring[..CIRCLE_SEGMENTS].iter().enumerate() {
            for b in &ring[i + 1..CIRCLE_SEGMENTS] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn every_circle_vertex_sits_at_the_radius() {
        for (lat, lon, r) in [
            (40.4168, -3.7038, 575.0),
            (0.0, 0.0, 50.0),
            (64.1466, -21.9426, 5_000.0),
            (-33.86, 151.21, 100_000.0),
        ] {
            for p in circle_ring(lat, lon, r) {
                let d = haversine_m([lon, lat], p);
                assert!((d - r).abs() < r * 1e-6 + 1e-3, "{lat},{lon} r={r}: {d}");
            }
        }
    }

    #[test]
    fn circle_ring_scales_longitude_by_latitude() {
        // Due east (a quarter turn) at 60°N spans twice the degrees of longitude
        // it would at the equator: metres, not degrees, stay constant.
        let q = CIRCLE_SEGMENTS / 4;
        let east_eq = circle_ring(0.0, 0.0, 1_000.0)[q];
        let east_60 = circle_ring(60.0, 0.0, 1_000.0)[q];
        assert!(
            (east_60[0] / east_eq[0] - 2.0).abs() < 1e-3,
            "{east_60:?} {east_eq:?}"
        );
        // North is the first vertex: same latitude step anywhere.
        let north = circle_ring(60.0, 10.0, 1_000.0)[0];
        assert!((north[0] - 10.0).abs() < 1e-9);
        assert!((north[1] - 60.0 - (1_000.0 / EARTH_R).to_degrees()).abs() < 1e-9);
    }

    #[test]
    fn label_point_ignores_the_closing_vertex() {
        let open = [[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0]];
        let mut closed = open.to_vec();
        closed.push(open[0]);
        assert_eq!(ring_label_point(&open), Some([1.0, 1.0]));
        assert_eq!(ring_label_point(&closed), Some([1.0, 1.0]));
        assert_eq!(ring_label_point(&[]), None);
        let c = ring_label_point(&circle_ring(40.0, -3.7, 500.0)).unwrap();
        assert!(
            (c[0] + 3.7).abs() < 1e-6 && (c[1] - 40.0).abs() < 1e-4,
            "{c:?}"
        );
    }
}
