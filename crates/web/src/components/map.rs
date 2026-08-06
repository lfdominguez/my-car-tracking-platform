use leptos::prelude::*;
use wasm_bindgen::prelude::*;

use crate::api::TripPoint;

#[wasm_bindgen(inline_js = r#"
let __tripSpeedUnit = 'km/h';
const TRIP_MAP_STYLE = 'https://tiles.openfreemap.org/styles/liberty';
const TRIP_MAP_PITCH = 48;
const TRIP_MAP_BEARING = 12;

function bearingDeg(lon1, lat1, lon2, lat2) {
  const toRad = (d) => d * Math.PI / 180;
  const toDeg = (r) => r * 180 / Math.PI;
  const φ1 = toRad(lat1);
  const φ2 = toRad(lat2);
  const Δλ = toRad(lon2 - lon1);
  const y = Math.sin(Δλ) * Math.cos(φ2);
  const x = Math.cos(φ1) * Math.sin(φ2) - Math.sin(φ1) * Math.cos(φ2) * Math.cos(Δλ);
  return (toDeg(Math.atan2(y, x)) + 360) % 360;
}

function haversineM(lon1, lat1, lon2, lat2) {
  const R = 6371000;
  const toRad = (d) => d * Math.PI / 180;
  const dLat = toRad(lat2 - lat1);
  const dLon = toRad(lon2 - lon1);
  const a = Math.sin(dLat / 2) ** 2 +
    Math.cos(toRad(lat1)) * Math.cos(toRad(lat2)) * Math.sin(dLon / 2) ** 2;
  // Clamp for floating-point overshoot so asin never yields NaN.
  return 2 * R * Math.asin(Math.min(1, Math.sqrt(Math.max(0, a))));
}

function parseTimeMs(iso) {
  if (!iso) return null;
  const t = Date.parse(iso);
  return Number.isFinite(t) ? t : null;
}

function pointSpeed(p) {
  const s = p.vehicle_speed_kph ?? p.engine_vel;
  return (s == null || !Number.isFinite(s)) ? null : s;
}

function pointRpm(p) {
  const r = p.vehicle_engine_rpm ?? p.engine_rpm;
  return (r == null || !Number.isFinite(r)) ? null : r;
}

function isStoppedSample(p, prev) {
  const s = pointSpeed(p);
  if (s != null) return s <= 2;
  if (!prev) return false;
  const d = haversineM(prev.lon, prev.lat, p.lon, p.lat);
  const t0 = parseTimeMs(prev.recorded_at);
  const t1 = parseTimeMs(p.recorded_at);
  if (t0 == null || t1 == null || t1 <= t0) return d < 3;
  const dtH = (t1 - t0) / 3600000;
  if (dtH <= 0) return d < 3;
  return (d / 1000) / dtH <= 2;
}

function formatDwell(ms) {
  if (!Number.isFinite(ms) || ms < 0) return '—';
  const totalSec = Math.round(ms / 1000);
  const m = Math.floor(totalSec / 60);
  const s = totalSec % 60;
  if (m >= 60) {
    const h = Math.floor(m / 60);
    const mm = m % 60;
    return `${h}h ${mm}m`;
  }
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}

function formatSpeedKph(v) {
  if (v == null || !Number.isFinite(Number(v))) return '—';
  return `${Math.round(Number(v))} ${__tripSpeedUnit}`;
}

export function setTripMapSpeedUnit(unit) {
  if (unit && typeof unit === 'string') {
    __tripSpeedUnit = unit;
  }
}

function formatRpm(v) {
  if (v == null || !Number.isFinite(Number(v))) return '—';
  return `${Math.round(Number(v))} rpm`;
}

function propNum(v) {
  if (v == null || v === '') return null;
  const n = Number(v);
  return Number.isFinite(n) ? n : null;
}

/** Contiguous ≥60s near-zero speed clusters → amber stop circles. */
function buildStopFeatures(points) {
  const features = [];
  let i = 0;
  while (i < points.length) {
    if (!isStoppedSample(points[i], i > 0 ? points[i - 1] : null)) {
      i += 1;
      continue;
    }
    const start = i;
    let j = i + 1;
    while (j < points.length && isStoppedSample(points[j], points[j - 1])) j += 1;
    const t0 = parseTimeMs(points[start].recorded_at);
    const t1 = parseTimeMs(points[j - 1].recorded_at);
    const dwell = (t0 != null && t1 != null) ? (t1 - t0) : 0;
    if (dwell >= 60000) {
      let lon = 0, lat = 0, n = 0;
      for (let k = start; k < j; k++) {
        if (Number.isFinite(points[k].lon) && Number.isFinite(points[k].lat)) {
          lon += points[k].lon;
          lat += points[k].lat;
          n += 1;
        }
      }
      if (n > 0) {
        features.push({
          type: 'Feature',
          properties: {
            dwell_ms: dwell,
            dwell_label: formatDwell(dwell),
            point_index: start,
            recorded_at: points[start].recorded_at || null
          },
          geometry: { type: 'Point', coordinates: [lon / n, lat / n] }
        });
      }
    }
    i = j;
  }
  return features;
}

/** Zoom-adaptive chevron/speed density (meters between markers + caps). */
function arrowDensityForZoom(zoom) {
  const z = Number.isFinite(zoom) ? zoom : 12;
  if (z <= 10) return { minSpacingM: 2200, maxArrows: 10, iconSize: 0.72, textSize: 10 };
  if (z <= 11) return { minSpacingM: 1600, maxArrows: 14, iconSize: 0.78, textSize: 10 };
  if (z <= 12) return { minSpacingM: 1000, maxArrows: 18, iconSize: 0.85, textSize: 11 };
  if (z <= 13) return { minSpacingM: 650, maxArrows: 26, iconSize: 0.92, textSize: 11 };
  if (z <= 14) return { minSpacingM: 380, maxArrows: 36, iconSize: 0.98, textSize: 12 };
  if (z <= 15) return { minSpacingM: 240, maxArrows: 48, iconSize: 1.02, textSize: 12 };
  if (z <= 16) return { minSpacingM: 150, maxArrows: 64, iconSize: 1.05, textSize: 12 };
  return { minSpacingM: 100, maxArrows: 80, iconSize: 1.08, textSize: 13 };
}

function buildArrowFeatures(points, stopFeatures, zoom) {
  const features = [];
  const stopZones = (stopFeatures || []).map((f) => ({
    lon: f.geometry.coordinates[0],
    lat: f.geometry.coordinates[1]
  }));
  const nearStop = (lon, lat) =>
    stopZones.some((z) => haversineM(lon, lat, z.lon, z.lat) < 18);

  const { minSpacingM, maxArrows } = arrowDensityForZoom(zoom);
  // Seed so the first eligible segment can place immediately after enough travel.
  let sinceLast = minSpacingM;
  let placed = 0;

  for (let i = 0; i < points.length - 1 && placed < maxArrows; i++) {
    const a = points[i];
    const b = points[i + 1];
    if (!a || !b || !Number.isFinite(a.lon) || !Number.isFinite(b.lon)) continue;
    const dist = haversineM(a.lon, a.lat, b.lon, b.lat);
    if (dist < 2) {
      sinceLast += dist;
      continue;
    }
    if (isStoppedSample(a, i > 0 ? points[i - 1] : null) && isStoppedSample(b, a)) {
      sinceLast += dist;
      continue;
    }
    sinceLast += dist;
    if (sinceLast < minSpacingM) continue;
    const midLon = (a.lon + b.lon) / 2;
    const midLat = (a.lat + b.lat) / 2;
    if (nearStop(midLon, midLat)) continue;
    const spd = pointSpeed(a) ?? pointSpeed(b);
    // Plain number; unit appended in layer text-field.
    const speedLabel = (spd != null && Number.isFinite(spd))
      ? `${Math.round(spd)} ${__tripSpeedUnit}`
      : '';
    features.push({
      type: 'Feature',
      properties: {
        bearing: bearingDeg(a.lon, a.lat, b.lon, b.lat),
        speed_label: speedLabel,
        speed_kph: spd
      },
      geometry: { type: 'Point', coordinates: [midLon, midLat] }
    });
    sinceLast = 0;
    placed += 1;
  }
  return features;
}

/** Rebuild chevrons for current zoom so zoom-out stays uncluttered. */
function refreshTripArrows(entry) {
  if (!entry || !entry.map) return;
  const map = entry.map;
  if (!map.isStyleLoaded() || !map.getSource('trip-arrows')) return;
  const zoom = map.getZoom();
  // Skip tiny zoom jitter (track whole zoom steps + a little).
  const zoomKey = Math.round(zoom * 4) / 4;
  if (entry._arrowZoomKey === zoomKey && entry._arrowsBuilt) return;
  entry._arrowZoomKey = zoomKey;
  entry._arrowsBuilt = true;
  const density = arrowDensityForZoom(zoom);
  const arrowFeatures = buildArrowFeatures(entry.points || [], entry.stopFeatures || [], zoom);
  map.getSource('trip-arrows').setData({ type: 'FeatureCollection', features: arrowFeatures });
  if (map.getLayer('trip-arrows')) {
    try {
      map.setLayoutProperty('trip-arrows', 'icon-size', density.iconSize);
      map.setLayoutProperty('trip-arrows', 'text-size', density.textSize);
    } catch (_) {}
  }
}

function bindArrowZoomRefresh(entry) {
  if (!entry || entry._arrowZoomBound) return;
  entry._arrowZoomBound = true;
  const schedule = () => {
    if (entry._arrowZoomTimer) clearTimeout(entry._arrowZoomTimer);
    entry._arrowZoomTimer = setTimeout(() => {
      // Skip if map was disposed while the timer was pending.
      if (!__tripMaps.get(entry._elId) || entry.map !== (__tripMaps.get(entry._elId) || {}).map) return;
      refreshTripArrows(entry);
    }, 80);
  };
  entry.map.on('zoom', schedule);
  entry.map.on('zoomend', () => {
    if (entry._arrowZoomTimer) clearTimeout(entry._arrowZoomTimer);
    if (!__tripMaps.has(entry._elId)) return;
    refreshTripArrows(entry);
  });
}

/**
 * Route as short segments colored by speed (trip-local min/max → speed_t 0..1).
 * Each segment carries hover/click telemetry (speed, rpm, time, point index).
 */
function buildSpeedLineFeatures(points, fallbackCoordinates) {
  const coords = [];
  (points || []).forEach((p, idx) => {
    if (p && Number.isFinite(p.lon) && Number.isFinite(p.lat)) {
      coords.push({
        lon: p.lon,
        lat: p.lat,
        speed: pointSpeed(p),
        rpm: pointRpm(p),
        recorded_at: p.recorded_at || null,
        point_index: idx
      });
    }
  });

  if (coords.length < 2) {
    const fc = fallbackCoordinates || [];
    if (fc.length < 2) {
      return { features: [], minSpeed: null, maxSpeed: null, hasSpeed: false };
    }
    return {
      features: [{
        type: 'Feature',
        properties: {
          speed_t: 0.45,
          speed_kph: null,
          rpm: null,
          recorded_at: null,
          point_index: 0
        },
        geometry: { type: 'LineString', coordinates: fc }
      }],
      minSpeed: null,
      maxSpeed: null,
      hasSpeed: false
    };
  }

  // Fill missing speeds from nearest neighbors so segments stay continuous.
  const speeds = coords.map((c) => c.speed);
  let last = null;
  for (let i = 0; i < speeds.length; i++) {
    if (speeds[i] != null) last = speeds[i];
    else if (last != null) speeds[i] = last;
  }
  last = null;
  for (let i = speeds.length - 1; i >= 0; i--) {
    if (speeds[i] != null) last = speeds[i];
    else if (last != null) speeds[i] = last;
  }

  const finite = speeds.filter((s) => s != null && Number.isFinite(s));
  let minSpeed = finite.length ? Math.min(...finite) : null;
  let maxSpeed = finite.length ? Math.max(...finite) : null;
  const span = (minSpeed != null && maxSpeed != null) ? (maxSpeed - minSpeed) : 0;
  const hasSpeed = finite.length > 0;

  const features = [];
  for (let i = 0; i < coords.length - 1; i++) {
    const a = coords[i];
    const b = coords[i + 1];
    const sa = speeds[i];
    const sb = speeds[i + 1];
    let speed = null;
    if (sa != null && sb != null) speed = (sa + sb) / 2;
    else speed = sa ?? sb;

    let speed_t = 0.45;
    if (hasSpeed && speed != null) {
      speed_t = span > 1e-6 ? (speed - minSpeed) / span : 0.5;
      speed_t = Math.max(0, Math.min(1, speed_t));
    }

    // Prefer start sample for click sync; average RPM when both present.
    let rpm = a.rpm;
    if (a.rpm != null && b.rpm != null) rpm = (a.rpm + b.rpm) / 2;
    else rpm = a.rpm ?? b.rpm;

    features.push({
      type: 'Feature',
      properties: {
        speed_t,
        speed_kph: speed,
        rpm,
        recorded_at: a.recorded_at,
        point_index: a.point_index
      },
      geometry: {
        type: 'LineString',
        coordinates: [[a.lon, a.lat], [b.lon, b.lat]]
      }
    });
  }

  return { features, minSpeed, maxSpeed, hasSpeed };
}

const TRAFFIC_LEVEL_COLORS = {
  free: '#2ecc71',
  light: '#a8e063',
  moderate: '#f1c40f',
  heavy: '#e67e22',
  jam: '#e74c3c',
  signal_stop: '#95a5a6'
};

function speedLinePaintColor() {
  // Prefer discrete congestion color when present; else speed gradient.
  return [
    'case',
    ['has', 'congestion_color'],
    ['to-color', ['get', 'congestion_color']],
    [
      'interpolate', ['linear'], ['coalesce', ['get', 'speed_t'], 0.45],
      0.0, '#1d4ed8',
      0.2, '#0891b2',
      0.4, '#16a34a',
      0.6, '#ca8a04',
      0.8, '#ea580c',
      1.0, '#dc2626'
    ]
  ];
}

function levelForTime(frames, tMs) {
  if (!frames || !frames.length || tMs == null) return null;
  for (let i = 0; i < frames.length; i++) {
    const f = frames[i];
    const a = Date.parse(f.t_start);
    const b = Date.parse(f.t_end);
    if (Number.isFinite(a) && Number.isFinite(b) && tMs >= a && tMs <= b) {
      return f.level || null;
    }
  }
  // nearest by start time
  let best = null;
  let bestD = Infinity;
  for (let i = 0; i < frames.length; i++) {
    const a = Date.parse(frames[i].t_start);
    if (!Number.isFinite(a)) continue;
    const d = Math.abs(a - tMs);
    if (d < bestD) { bestD = d; best = frames[i].level; }
  }
  return best;
}

function applyTrafficColors(features, frames) {
  if (!frames || !frames.length || !features || !features.length) return false;
  let any = false;
  for (let i = 0; i < features.length; i++) {
    const t = parseTimeMs(features[i].properties && features[i].properties.recorded_at);
    const level = levelForTime(frames, t);
    if (level && TRAFFIC_LEVEL_COLORS[level]) {
      features[i].properties.congestion_color = TRAFFIC_LEVEL_COLORS[level];
      features[i].properties.traffic_level = level;
      any = true;
    }
  }
  return any;
}

function ensureMapImages(map) {
  if (!map.hasImage('flow-chevron')) {
    const size = 64;
    const canvas = document.createElement('canvas');
    canvas.width = size;
    canvas.height = size;
    const ctx = canvas.getContext('2d');
    ctx.clearRect(0, 0, size, size);
    ctx.beginPath();
    ctx.moveTo(size * 0.5, size * 0.06);
    ctx.lineTo(size * 0.92, size * 0.86);
    ctx.lineTo(size * 0.5, size * 0.62);
    ctx.lineTo(size * 0.08, size * 0.86);
    ctx.closePath();
    // High-contrast on Liberty (light) basemap
    ctx.fillStyle = '#0b1220';
    ctx.strokeStyle = '#ffffff';
    ctx.lineWidth = 3;
    ctx.lineJoin = 'round';
    ctx.fill();
    ctx.stroke();
    map.addImage('flow-chevron', ctx.getImageData(0, 0, size, size), { pixelRatio: 2 });
  }
  // Amber circle for dwell stops (replaces square).
  if (!map.hasImage('stop-circle')) {
    const size = 56;
    const canvas = document.createElement('canvas');
    canvas.width = size;
    canvas.height = size;
    const ctx = canvas.getContext('2d');
    ctx.clearRect(0, 0, size, size);
    const cx = size / 2;
    const cy = size / 2;
    const r = size * 0.34;
    ctx.beginPath();
    ctx.arc(cx, cy, r, 0, Math.PI * 2);
    ctx.fillStyle = '#f59e0b';
    ctx.strokeStyle = '#0f172a';
    ctx.lineWidth = 3;
    ctx.fill();
    ctx.stroke();
    map.addImage('stop-circle', ctx.getImageData(0, 0, size, size), { pixelRatio: 2 });
  }
  if (!map.hasImage('select-pin')) {
    const size = 48;
    const canvas = document.createElement('canvas');
    canvas.width = size;
    canvas.height = size;
    const ctx = canvas.getContext('2d');
    ctx.clearRect(0, 0, size, size);
    // Pink ring matching chart markLine.
    ctx.beginPath();
    ctx.arc(size / 2, size / 2, size * 0.32, 0, Math.PI * 2);
    ctx.fillStyle = 'rgba(244, 114, 182, 0.35)';
    ctx.fill();
    ctx.beginPath();
    ctx.arc(size / 2, size / 2, size * 0.20, 0, Math.PI * 2);
    ctx.fillStyle = '#fdf2f8';
    ctx.strokeStyle = '#be185d';
    ctx.lineWidth = 3;
    ctx.fill();
    ctx.stroke();
    map.addImage('select-pin', ctx.getImageData(0, 0, size, size), { pixelRatio: 2 });
  }
}

function updateSpeedLegend(minSpeed, maxSpeed, hasSpeed) {
  const minEl = document.getElementById('trip-speed-min');
  const maxEl = document.getElementById('trip-speed-max');
  const bar = document.getElementById('trip-speed-bar');
  if (minEl) minEl.textContent = hasSpeed ? formatSpeedKph(minSpeed) : '—';
  if (maxEl) maxEl.textContent = hasSpeed ? formatSpeedKph(maxSpeed) : '—';
  if (bar) bar.classList.toggle('is-empty', !hasSpeed);
}

function setSelectionClearVisible(visible) {
  const btn = document.getElementById('trip-selection-clear');
  if (btn) btn.hidden = !visible;
}

function fitTripBounds(map, coordinates) {
  if (!coordinates || coordinates.length === 0) return;
  const bounds = coordinates.reduce(
    (b, c) => b.extend(c),
    new maplibregl.LngLatBounds(coordinates[0], coordinates[0])
  );
  map.fitBounds(bounds, {
    padding: 56,
    maxZoom: 16,
    pitch: TRIP_MAP_PITCH,
    bearing: TRIP_MAP_BEARING
  });
}

function emptyFc() {
  return { type: 'FeatureCollection', features: [] };
}

function selectionFc(lon, lat) {
  if (!Number.isFinite(lon) || !Number.isFinite(lat)) return emptyFc();
  return {
    type: 'FeatureCollection',
    features: [{
      type: 'Feature',
      properties: {},
      geometry: { type: 'Point', coordinates: [lon, lat] }
    }]
  };
}


function repairArrowLayerLayout(map) {
  if (!map.getLayer('trip-arrows')) return;
  try {
    map.setLayoutProperty('trip-arrows', 'icon-image', 'flow-chevron');
    map.setLayoutProperty('trip-arrows', 'icon-size', 1.05);
    map.setLayoutProperty('trip-arrows', 'icon-rotate', ['get', 'bearing']);
    map.setLayoutProperty('trip-arrows', 'icon-rotation-alignment', 'map');
    map.setLayoutProperty('trip-arrows', 'icon-allow-overlap', false);
    map.setLayoutProperty('trip-arrows', 'icon-ignore-placement', false);
    map.setLayoutProperty('trip-arrows', 'icon-padding', 4);
    map.setLayoutProperty('trip-arrows', 'text-field', [
      'case',
      ['>', ['length', ['to-string', ['get', 'speed_label']]], 0],
      ['to-string', ['get', 'speed_label']],
      ''
    ]);
    map.setLayoutProperty('trip-arrows', 'text-size', 12);
    // Must match OpenFreeMap glyph stacks exactly (single Noto face).
    map.setLayoutProperty('trip-arrows', 'text-font', ['Noto Sans Regular']);
    map.setLayoutProperty('trip-arrows', 'text-offset', [0, 1.55]);
    map.setLayoutProperty('trip-arrows', 'text-anchor', 'top');
    map.setLayoutProperty('trip-arrows', 'text-allow-overlap', false);
    map.setLayoutProperty('trip-arrows', 'text-ignore-placement', false);
    map.setLayoutProperty('trip-arrows', 'text-optional', true);
    map.setLayoutProperty('trip-arrows', 'text-padding', 2);
    map.setPaintProperty('trip-arrows', 'icon-opacity', 0.98);
    map.setPaintProperty('trip-arrows', 'text-color', '#0f172a');
    map.setPaintProperty('trip-arrows', 'text-halo-color', '#ffffff');
    map.setPaintProperty('trip-arrows', 'text-halo-width', 1.8);
  } catch (err) {
    console.warn('repairArrowLayerLayout failed', err);
  }
}

function addTripLayers(map, lineFc, arrowsFc, stopsFc) {
  ensureMapImages(map);

  if (!map.getSource('trip')) {
    map.addSource('trip', { type: 'geojson', data: lineFc });
  } else {
    map.getSource('trip').setData(lineFc);
  }
  if (!map.getSource('trip-arrows')) {
    map.addSource('trip-arrows', { type: 'geojson', data: arrowsFc });
  } else {
    map.getSource('trip-arrows').setData(arrowsFc);
  }
  if (!map.getSource('trip-stops')) {
    map.addSource('trip-stops', { type: 'geojson', data: stopsFc });
  } else {
    map.getSource('trip-stops').setData(stopsFc);
  }
  if (!map.getSource('trip-selection')) {
    map.addSource('trip-selection', { type: 'geojson', data: emptyFc() });
  }

  // If an older SPA session added trip-arrows with a bad fontstack, fix in place.
  repairArrowLayerLayout(map);

  if (!map.getLayer('trip-line-halo')) {
    map.addLayer({
      id: 'trip-line-halo',
      type: 'line',
      source: 'trip',
      layout: { 'line-join': 'round', 'line-cap': 'round' },
      paint: {
        'line-color': '#0f172a',
        'line-width': 9,
        'line-opacity': 0.35
      }
    });
  }
  if (!map.getLayer('trip-line')) {
    map.addLayer({
      id: 'trip-line',
      type: 'line',
      source: 'trip',
      layout: { 'line-join': 'round', 'line-cap': 'round' },
      paint: {
        'line-color': speedLinePaintColor(),
        'line-width': 5,
        'line-opacity': 0.95
      }
    });
  }
  // Invisible wider hit target for easier hover/click on the route.
  if (!map.getLayer('trip-line-hit')) {
    map.addLayer({
      id: 'trip-line-hit',
      type: 'line',
      source: 'trip',
      layout: { 'line-join': 'round', 'line-cap': 'round' },
      paint: {
        'line-color': '#000000',
        'line-width': 18,
        'line-opacity': 0
      }
    });
  }
  if (!map.getLayer('trip-arrows')) {
    // OpenFreeMap Liberty only ships Noto Sans* glyphs. A multi-font stack
    // (e.g. Open Sans / Arial) requests a missing fontstack and MapLibre drops
    // the entire symbol layer — which hid chevrons + speed after labels were added.
    const arrowLayoutWithText = {
      'icon-image': 'flow-chevron',
      'icon-size': 1.05,
      'icon-rotate': ['get', 'bearing'],
      'icon-rotation-alignment': 'map',
      // Prefer collision over forced stacking; density is primary via zoom spacing.
      'icon-allow-overlap': false,
      'icon-ignore-placement': false,
      'icon-padding': 4,
      'symbol-spacing': 250,
      'text-field': [
        'case',
        ['>', ['length', ['to-string', ['get', 'speed_label']]], 0],
        ['to-string', ['get', 'speed_label']],
        ''
      ],
      'text-size': 13,
      'text-font': ['Noto Sans Regular'],
      'text-offset': [0, 1.55],
      'text-anchor': 'top',
      'text-allow-overlap': false,
      'text-ignore-placement': false,
      'text-optional': true,
      'text-padding': 2
    };
    const arrowPaint = {
      'icon-opacity': 0.98,
      'text-color': '#0f172a',
      'text-halo-color': '#ffffff',
      'text-halo-width': 1.8,
      'text-halo-blur': 0.3
    };
    try {
      map.addLayer({
        id: 'trip-arrows',
        type: 'symbol',
        source: 'trip-arrows',
        layout: arrowLayoutWithText,
        paint: arrowPaint
      });
    } catch (err) {
      console.warn('trip-arrows with text failed, falling back to icons only', err);
      try {
        if (map.getLayer('trip-arrows')) map.removeLayer('trip-arrows');
      } catch (_) {}
      map.addLayer({
        id: 'trip-arrows',
        type: 'symbol',
        source: 'trip-arrows',
        layout: {
          'icon-image': 'flow-chevron',
          'icon-size': 1.05,
          'icon-rotate': ['get', 'bearing'],
          'icon-rotation-alignment': 'map',
          'icon-allow-overlap': false,
          'icon-ignore-placement': false,
          'icon-padding': 4
        },
        paint: { 'icon-opacity': 0.98 }
      });
    }
  }
  if (!map.getLayer('trip-stops')) {
    map.addLayer({
      id: 'trip-stops',
      type: 'symbol',
      source: 'trip-stops',
      layout: {
        'icon-image': 'stop-circle',
        'icon-size': 1.25,
        'icon-allow-overlap': true,
        'icon-ignore-placement': true,
        'icon-padding': 0
      },
      paint: { 'icon-opacity': 0.98 }
    });
  }
  if (!map.getLayer('trip-selection')) {
    map.addLayer({
      id: 'trip-selection',
      type: 'symbol',
      source: 'trip-selection',
      layout: {
        'icon-image': 'select-pin',
        'icon-size': 1.05,
        'icon-allow-overlap': true,
        'icon-ignore-placement': true,
        'icon-padding': 0
      },
      paint: { 'icon-opacity': 1 }
    });
  }
}

function routeHoverHtml(props) {
  const speed = formatSpeedKph(propNum(props && props.speed_kph));
  const rpm = formatRpm(propNum(props && props.rpm));
  return `<div class="trip-route-popup-inner">
    <div class="trip-route-popup-row"><span>Velocity</span><strong>${speed}</strong></div>
    <div class="trip-route-popup-row"><span>RPM</span><strong>${rpm}</strong></div>
  </div>`;
}

function bindTripInteractions(entry) {
  if (entry.bound) return;
  entry.bound = true;
  const map = entry.map;
  const popup = entry.popup;
  const stopPopup = entry.stopPopup;

  const clearLocalSelection = () => {
    entry.selection = null;
    if (map.getSource('trip-selection')) {
      map.getSource('trip-selection').setData(emptyFc());
    }
    setSelectionClearVisible(false);
  };

  const applyLocalSelection = (sel) => {
    entry.selection = sel;
    if (map.getSource('trip-selection')) {
      map.getSource('trip-selection').setData(selectionFc(sel.lon, sel.lat));
    }
    setSelectionClearVisible(true);
  };

  entry.clearSelection = () => {
    clearLocalSelection();
    try {
      if (window.__tripTelemetry && window.__tripTelemetry.clearSelection) {
        window.__tripTelemetry.clearSelection();
      }
    } catch (_) {}
  };

  entry.setSelectionFromProps = (props, lngLat) => {
    const iso = props && props.recorded_at ? String(props.recorded_at) : null;
    const pointIndex = propNum(props && props.point_index);
    if (!iso && pointIndex == null) return;

    // Toggle off if clicking the same sample again.
    if (
      entry.selection &&
      ((iso && entry.selection.iso === iso) ||
        (pointIndex != null && entry.selection.point_index === pointIndex))
    ) {
      entry.clearSelection();
      return;
    }

    let lon = lngLat && lngLat.lng;
    let lat = lngLat && lngLat.lat;
    // Snap marker to the sample point when we still have points loaded.
    if (entry.points && pointIndex != null && entry.points[pointIndex]) {
      const p = entry.points[pointIndex];
      if (Number.isFinite(p.lon) && Number.isFinite(p.lat)) {
        lon = p.lon;
        lat = p.lat;
      }
    }

    applyLocalSelection({
      iso,
      point_index: pointIndex,
      lon,
      lat
    });

    try {
      if (window.__tripTelemetry && window.__tripTelemetry.selectByTime && iso) {
        window.__tripTelemetry.selectByTime(iso, pointIndex);
      }
    } catch (_) {}
  };

  // --- Stop hover (priority over route when both under cursor) ---
  map.on('mouseenter', 'trip-stops', (e) => {
    entry.overStop = true;
    map.getCanvas().style.cursor = 'pointer';
    popup.remove();
    const f = e.features && e.features[0];
    if (!f) return;
    const coords = f.geometry.coordinates.slice();
    const label = (f.properties && f.properties.dwell_label) || 'Stop';
    stopPopup
      .setLngLat(coords)
      .setHTML(`<div class="trip-stop-popup-inner"><strong>Stopped</strong><span>${label}</span></div>`)
      .addTo(map);
  });
  map.on('mouseleave', 'trip-stops', () => {
    entry.overStop = false;
    stopPopup.remove();
    if (!entry.overRoute) map.getCanvas().style.cursor = '';
  });

  // --- Route hover: velocity + RPM ---
  const onRouteMove = (e) => {
    if (entry.overStop) return;
    entry.overRoute = true;
    map.getCanvas().style.cursor = 'pointer';
    const f = e.features && e.features[0];
    if (!f) return;
    popup
      .setLngLat(e.lngLat)
      .setHTML(routeHoverHtml(f.properties || {}))
      .addTo(map);
  };
  const onRouteLeave = () => {
    entry.overRoute = false;
    popup.remove();
    if (!entry.overStop) map.getCanvas().style.cursor = '';
  };
  // Interact only via the wide hit layer so line+hit don't double-fire (toggle off).
  map.on('mousemove', 'trip-line-hit', onRouteMove);
  map.on('mouseenter', 'trip-line-hit', onRouteMove);
  map.on('mouseleave', 'trip-line-hit', onRouteLeave);

  // --- Click route → sticky chart selection ---
  const onRouteClick = (e) => {
    if (e.originalEvent) {
      e.originalEvent.stopPropagation();
      // Prevent the map-level click clear handler in the same gesture.
      entry._ignoreNextMapClick = true;
    }
    const f = e.features && e.features[0];
    if (!f) return;
    entry.setSelectionFromProps(f.properties || {}, e.lngLat);
  };
  map.on('click', 'trip-line-hit', onRouteClick);

  // Click stop → select that dwell start time as well.
  map.on('click', 'trip-stops', (e) => {
    if (e.originalEvent) {
      e.originalEvent.stopPropagation();
      entry._ignoreNextMapClick = true;
    }
    const f = e.features && e.features[0];
    if (!f) return;
    entry.setSelectionFromProps(f.properties || {}, {
      lng: f.geometry.coordinates[0],
      lat: f.geometry.coordinates[1]
    });
  });

  // Empty map click clears selection.
  map.on('click', () => {
    if (entry._ignoreNextMapClick) {
      entry._ignoreNextMapClick = false;
      return;
    }
    entry.clearSelection();
  });

  // Escape clears.
  if (!entry.onKey) {
    entry.onKey = (ev) => {
      if (ev.key === 'Escape') entry.clearSelection();
    };
    window.addEventListener('keydown', entry.onKey);
  }

  // Legend clear button.
  const btn = document.getElementById('trip-selection-clear');
  if (btn && !btn.__tripBound) {
    btn.__tripBound = true;
    btn.addEventListener('click', (ev) => {
      ev.preventDefault();
      entry.clearSelection();
    });
  }

  // If charts clear selection (future), drop map pin.
  if (!entry.onTelemetryClear) {
    entry.onTelemetryClear = () => {
      clearLocalSelection();
    };
    window.addEventListener('trip-telemetry-clear', entry.onTelemetryClear);
  }
}

function restoreSelectionMarker(entry) {
  if (!entry.selection) {
    setSelectionClearVisible(false);
    if (entry.map.getSource('trip-selection')) {
      entry.map.getSource('trip-selection').setData(emptyFc());
    }
    return;
  }
  const sel = entry.selection;
  if (entry.map.getSource('trip-selection')) {
    entry.map.getSource('trip-selection').setData(selectionFc(sel.lon, sel.lat));
  }
  setSelectionClearVisible(true);
}

const __tripMaps = new Map();

function destroyTripMapEntry(elId, entry) {
  if (!entry) return;
  try { if (entry._arrowZoomTimer) clearTimeout(entry._arrowZoomTimer); } catch (_) {}
  try { if (entry.onKey) window.removeEventListener('keydown', entry.onKey); } catch (_) {}
  try { if (entry.onTelemetryClear) window.removeEventListener('trip-telemetry-clear', entry.onTelemetryClear); } catch (_) {}
  try { entry.popup && entry.popup.remove(); } catch (_) {}
  try { entry.stopPopup && entry.stopPopup.remove(); } catch (_) {}
  try { entry.map && entry.map.remove(); } catch (_) {}
  try {
    const btn = document.getElementById('trip-selection-clear');
    if (btn) btn.__tripBound = false;
  } catch (_) {}
  __tripMaps.delete(elId);
}

/** Tear down MapLibre instance when the Leptos map component unmounts. */
export function disposeTripMap(elId) {
  const entry = __tripMaps.get(elId);
  if (!entry) return;
  destroyTripMapEntry(elId, entry);
  try {
    const el = document.getElementById(elId);
    if (el) el.innerHTML = '';
  } catch (_) {}
}

export function renderTripMap(elId, geojson, pointsJson, trafficJson) {
  const el = document.getElementById(elId);
  if (!el || !window.maplibregl) return;

  let points = [];
  try {
    points = typeof pointsJson === 'string' ? JSON.parse(pointsJson) : (pointsJson || []);
  } catch (_) {
    points = [];
  }
  if (!Array.isArray(points)) points = [];

  let trafficFrames = [];
  try {
    trafficFrames = typeof trafficJson === 'string' ? JSON.parse(trafficJson) : (trafficJson || []);
  } catch (_) {
    trafficFrames = [];
  }
  if (!Array.isArray(trafficFrames)) trafficFrames = [];

  let coordinates = points
    .filter((p) => p && Number.isFinite(p.lon) && Number.isFinite(p.lat))
    .map((p) => [p.lon, p.lat]);
  if (coordinates.length < 2 && geojson && Array.isArray(geojson.coordinates)) {
    coordinates = geojson.coordinates;
  }

  const speedBuilt = buildSpeedLineFeatures(points, coordinates);
  const usedTraffic = applyTrafficColors(speedBuilt.features, trafficFrames);
  const lineFc = { type: 'FeatureCollection', features: speedBuilt.features };
  const stopFeatures = buildStopFeatures(points);
  const stopsFc = { type: 'FeatureCollection', features: stopFeatures };
  // Initial arrows use a mid zoom; refined once the map reports real zoom.
  const initialZoom = (() => {
    const ex = __tripMaps.get(elId);
    if (ex && ex.map && typeof ex.map.getZoom === 'function') {
      try { return ex.map.getZoom(); } catch (_) {}
    }
    return 13;
  })();
  const arrowFeatures = buildArrowFeatures(points, stopFeatures, initialZoom);
  const arrowsFc = { type: 'FeatureCollection', features: arrowFeatures };

  if (usedTraffic) {
    const minEl = document.getElementById('trip-speed-min');
    const maxEl = document.getElementById('trip-speed-max');
    const bar = document.getElementById('trip-speed-bar');
    if (minEl) minEl.textContent = 'Free';
    if (maxEl) maxEl.textContent = 'Jam';
    if (bar) {
      bar.classList.remove('is-empty');
      bar.style.background = 'linear-gradient(90deg,#2ecc71,#a8e063,#f1c40f,#e67e22,#e74c3c)';
    }
  } else {
    updateSpeedLegend(speedBuilt.minSpeed, speedBuilt.maxSpeed, speedBuilt.hasSpeed);
  }

  let existing = __tripMaps.get(elId);
  // Recreate when style changes OR the DOM container was replaced (Leptos remount).
  const staleContainer = existing && existing.container && existing.container !== el;
  if (existing && (existing.style !== TRIP_MAP_STYLE || staleContainer)) {
    destroyTripMapEntry(elId, existing);
    existing = null;
    try { el.innerHTML = ''; } catch (_) {}
  }

  if (existing) {
    existing._elId = elId;
    existing.container = el;
    existing.points = points;
    existing.stopFeatures = stopFeatures;
    existing._arrowsBuilt = false;
    existing._arrowZoomKey = null;
    const map = existing.map;
    const setData = () => {
      if (__tripMaps.get(elId) !== existing) return;
      addTripLayers(map, lineFc, arrowsFc, stopsFc);
      bindTripInteractions(existing);
      bindArrowZoomRefresh(existing);
      refreshTripArrows(existing);
      restoreSelectionMarker(existing);
      fitTripBounds(map, coordinates);
    };
    if (map.isStyleLoaded()) setData();
    else map.once('load', setData);
    return;
  }

  el.innerHTML = '';
  const map = new maplibregl.Map({
    container: el,
    style: TRIP_MAP_STYLE,
    center: coordinates[0] || [0, 0],
    zoom: coordinates.length ? 13 : 1,
    pitch: TRIP_MAP_PITCH,
    bearing: TRIP_MAP_BEARING,
    attributionControl: true
  });
  map.addControl(
    new maplibregl.NavigationControl({ showCompass: true, visualizePitch: true }),
    'top-right'
  );

  const popup = new maplibregl.Popup({
    closeButton: false,
    closeOnClick: false,
    offset: 14,
    className: 'trip-route-popup'
  });
  const stopPopup = new maplibregl.Popup({
    closeButton: false,
    closeOnClick: false,
    offset: 14,
    className: 'trip-stop-popup'
  });

  const entry = {
    map,
    container: el,
    popup,
    stopPopup,
    style: TRIP_MAP_STYLE,
    points,
    stopFeatures,
    selection: null,
    bound: false,
    overStop: false,
    overRoute: false,
    _arrowZoomBound: false,
    _arrowZoomTimer: null,
    _arrowsBuilt: false,
    _arrowZoomKey: null
  };
  entry._elId = elId;
  __tripMaps.set(elId, entry);

  map.on('load', () => {
    // Component may have unmounted before style finished loading.
    if (__tripMaps.get(elId) !== entry) return;
    addTripLayers(map, lineFc, arrowsFc, stopsFc);
    bindTripInteractions(entry);
    bindArrowZoomRefresh(entry);
    refreshTripArrows(entry);
    restoreSelectionMarker(entry);
    fitTripBounds(map, coordinates);
  });
}
"#)]
extern "C" {
    fn renderTripMap(el_id: &str, geojson: &JsValue, points_json: &str, traffic_json: &str);
    fn setTripMapSpeedUnit(unit: &str);
    fn disposeTripMap(el_id: &str);
}

#[component]
pub fn TripMap(
    geojson: Signal<Option<serde_json::Value>>,
    #[prop(into)] points: Signal<Vec<TripPoint>>,
    #[prop(into, optional)] traffic_frames: Option<Signal<Vec<crate::api::TripTrafficFrame>>>,
) -> impl IntoView {
    let id = "trip-map";
    let prefs = crate::units::use_unit_prefs();
    let traffic_frames = traffic_frames.unwrap_or_else(|| Signal::derive(|| Vec::new()));

    // Always tear down MapLibre when this component leaves the tree so a remount
    // does not reuse a map bound to a disposed DOM node / reactive scope.
    on_cleanup(move || {
        disposeTripMap(id);
    });

    Effect::new(move |_| {
        // Prefer try_* so a late effect tick after unmount cannot panic the SPA.
        let Some(prefs_now) = prefs.try_get() else {
            return;
        };
        setTripMapSpeedUnit(prefs_now.labels.speed);
        let Some(gj) = geojson.try_get() else {
            return;
        };
        let Some(pts) = points.try_get() else {
            return;
        };
        let frames = traffic_frames.try_get().unwrap_or_default();
        // Need either a line payload or enough points to draw.
        if gj.is_none() && pts.len() < 2 {
            return;
        }
        let gj_val = gj.unwrap_or_else(|| {
            serde_json::json!({
                "type": "LineString",
                "coordinates": []
            })
        });
        let Ok(js) = serde_wasm_bindgen_compat(&gj_val) else {
            return;
        };
        let pts_json = serde_json::to_string(&pts).unwrap_or_else(|_| "[]".into());
        let traffic_json = serde_json::to_string(&frames).unwrap_or_else(|_| "[]".into());
        renderTripMap(id, &js, &pts_json, &traffic_json);
    });
    view! { <div id=id class="map"></div> }
}

fn serde_wasm_bindgen_compat(v: &serde_json::Value) -> Result<JsValue, String> {
    let s = serde_json::to_string(v).map_err(|e| e.to_string())?;
    js_sys::JSON::parse(&s).map_err(|e| format!("{e:?}"))
}

#[wasm_bindgen(inline_js = r#"
const ROUTE_OPT_STYLE = 'https://tiles.openfreemap.org/styles/liberty';
// Saturated solid colors for *your* recorded path variants.
const VARIANT_COLORS = ['#0077ff', '#00c853', '#ffd600', '#00e5ff', '#304ffe', '#76ff03'];
// Distinct magenta/rose family for OpenRouteService alternatives (always dashed).
const ORS_COLORS = ['#ff2d95', '#ff9100', '#d500f9', '#ff1744'];
let __routeOptMap = null;
let __routeOptHost = null;
let __routeOptPopup = null;

function routeOptDisplayLabel(props) {
  if (!props) return '';
  const raw = props.label || '';
  if (props.kind === 'ors') {
    const name = raw.replace(/^ORS\s+/i, '') || 'alternative';
    return 'Router · ' + name;
  }
  return 'Variant · ' + (raw || 'path');
}

function routeOptColorFor(props) {
  const palette = props && props.kind === 'ors' ? ORS_COLORS : VARIANT_COLORS;
  let idx = Number(props && props.color_index);
  if (!Number.isFinite(idx) || idx < 0) idx = 0;
  idx = Math.floor(idx) % palette.length;
  if (idx < 0) idx += palette.length;
  return palette[idx] || palette[0];
}

function enrichRouteOptGeojson(data) {
  const features = (data && data.features) ? data.features : [];
  // Assign concrete hex colors here. MapLibre `at`/`literal` color expressions
  // were failing at paint time, leaving only the dark halo (looks black/transparent).
  const out = features.map((f) => {
    const props = Object.assign({}, f.properties || {});
    props.display_label = routeOptDisplayLabel(props);
    props.kind_label = props.kind === 'ors' ? 'OpenRouteService' : 'Your path';
    props.route_color = routeOptColorFor(props);
    return {
      type: 'Feature',
      properties: props,
      geometry: f.geometry,
    };
  });
  return { type: 'FeatureCollection', features: out };
}

export function disposeRouteOptMap() {
  try {
    if (__routeOptPopup) {
      __routeOptPopup.remove();
    }
  } catch (e) {}
  __routeOptPopup = null;
  try {
    if (__routeOptMap) {
      __routeOptMap.remove();
    }
  } catch (e) {}
  __routeOptMap = null;
  __routeOptHost = null;
}

function ensureRouteOptLayers(map) {
  if (map.getSource('route-opt')) return;

  map.addSource('route-opt', {
    type: 'geojson',
    data: { type: 'FeatureCollection', features: [] },
  });

  // ORS first (under variants): dashed hot family
  map.addLayer({
    id: 'route-opt-ors-halo',
    type: 'line',
    source: 'route-opt',
    filter: ['==', ['get', 'kind'], 'ors'],
    layout: { 'line-cap': 'round', 'line-join': 'round' },
    paint: {
      // Soft dark outline only — color comes from the dashed line above it
      'line-color': '#111827',
      'line-width': 6,
      'line-opacity': 0.22,
      'line-dasharray': [2.5, 1.6],
    },
  });
  map.addLayer({
    id: 'route-opt-ors-line',
    type: 'line',
    source: 'route-opt',
    filter: ['==', ['get', 'kind'], 'ors'],
    layout: { 'line-cap': 'butt', 'line-join': 'round' },
    paint: {
      // Pre-baked hex on each feature (see enrichRouteOptGeojson)
      'line-color': ['to-color', ['get', 'route_color']],
      'line-width': 3.25,
      'line-opacity': 0.95,
      // Long dashes read clearly against solid variants
      'line-dasharray': [4, 2.5],
    },
  });

  // Your variants on top: thick solid
  map.addLayer({
    id: 'route-opt-variant-halo',
    type: 'line',
    source: 'route-opt',
    filter: ['==', ['get', 'kind'], 'variant'],
    layout: { 'line-cap': 'round', 'line-join': 'round' },
    paint: {
      'line-color': '#111827',
      'line-width': 10,
      'line-opacity': 0.28,
    },
  });
  map.addLayer({
    id: 'route-opt-variant-line',
    type: 'line',
    source: 'route-opt',
    filter: ['==', ['get', 'kind'], 'variant'],
    layout: { 'line-cap': 'round', 'line-join': 'round' },
    paint: {
      'line-color': ['to-color', ['get', 'route_color']],
      'line-width': 6.5,
      'line-opacity': 1.0,
    },
  });

  // On-path labels (kind · name)
  map.addLayer({
    id: 'route-opt-labels',
    type: 'symbol',
    source: 'route-opt',
    layout: {
      'symbol-placement': 'line-center',
      'text-field': ['get', 'display_label'],
      'text-size': 13,
      'text-font': ['Noto Sans Regular'],
      'text-max-angle': 30,
      'text-allow-overlap': false,
      'text-ignore-placement': false,
      'symbol-spacing': 320,
    },
    paint: {
      'text-color': ['to-color', ['coalesce', ['get', 'route_color'], '#1e3a8a']],
      'text-halo-color': 'rgba(255,255,255,0.96)',
      'text-halo-width': 2.0,
      'text-halo-blur': 0.4,
    },
  });

  // Wide invisible hit layer for hover
  map.addLayer({
    id: 'route-opt-hit',
    type: 'line',
    source: 'route-opt',
    paint: {
      'line-color': '#000000',
      'line-width': 16,
      'line-opacity': 0.01,
    },
  });

  __routeOptPopup = new maplibregl.Popup({
    closeButton: false,
    closeOnClick: false,
    offset: 10,
    className: 'trip-route-popup route-opt-popup',
  });

  map.on('mousemove', 'route-opt-hit', (e) => {
    if (!e.features || !e.features.length) return;
    map.getCanvas().style.cursor = 'pointer';
    const f = e.features[0];
    const props = f.properties || {};
    const kind = props.kind === 'ors' ? 'OpenRouteService alternative' : 'Your path variant';
    const name = props.label || '';
    const title = props.kind === 'ors'
      ? (name.replace(/^ORS\s+/i, '') || 'Router alt')
      : (name || 'Variant');
    const html =
      '<div class="route-opt-popup-inner">' +
      '<span class="route-opt-popup-kind ' + (props.kind === 'ors' ? 'is-ors' : 'is-variant') + '">' +
      kind +
      '</span>' +
      '<strong>' + title + '</strong>' +
      '<span class="route-opt-popup-style">' +
      (props.kind === 'ors' ? 'Dashed line · router estimate' : 'Solid line · recorded trips') +
      '</span></div>';
    __routeOptPopup.setLngLat(e.lngLat).setHTML(html).addTo(map);
  });
  map.on('mouseleave', 'route-opt-hit', () => {
    map.getCanvas().style.cursor = '';
    if (__routeOptPopup) __routeOptPopup.remove();
  });
}

export function mountRouteOptMap(host, geojson) {
  if (!host || typeof maplibregl === 'undefined') return;
  if (__routeOptMap && __routeOptHost !== host) {
    disposeRouteOptMap();
  }
  const raw = typeof geojson === 'string' ? JSON.parse(geojson) : geojson;
  const data = enrichRouteOptGeojson(raw || {});
  if (!__routeOptMap) {
    __routeOptMap = new maplibregl.Map({
      container: host,
      style: ROUTE_OPT_STYLE,
      center: [0, 20],
      zoom: 2,
      pitch: 40,
      bearing: 8,
      attributionControl: true,
    });
    __routeOptMap.addControl(new maplibregl.NavigationControl({ visualizePitch: true }), 'top-right');
    __routeOptHost = host;
  }
  const map = __routeOptMap;
  const apply = () => {
    ensureRouteOptLayers(map);
    const src = map.getSource('route-opt');
    if (src) src.setData(data);
    try {
      const bounds = new maplibregl.LngLatBounds();
      let any = false;
      (data.features || []).forEach((f) => {
        const coords = f.geometry && f.geometry.coordinates;
        if (!coords) return;
        coords.forEach((c) => {
          if (Array.isArray(c) && c.length >= 2) {
            bounds.extend([c[0], c[1]]);
            any = true;
          }
        });
      });
      if (any) map.fitBounds(bounds, { padding: 56, maxZoom: 14, duration: 0 });
    } catch (e) {}
  };
  if (map.isStyleLoaded()) apply();
  else map.once('load', apply);
}
"#)]
extern "C" {
    #[wasm_bindgen(js_name = mountRouteOptMap)]
    fn mount_route_opt_map_js(host: &web_sys::HtmlElement, geojson: &JsValue);

    #[wasm_bindgen(js_name = disposeRouteOptMap)]
    fn dispose_route_opt_map_js();
}

/// Mount corridor comparison map (variants + ORS lines).
pub fn mount_route_opt_map(host: &web_sys::HtmlElement, geo: &serde_json::Value) {
    if let Ok(js) = serde_wasm_bindgen_compat(geo) {
        mount_route_opt_map_js(host, &js);
    }
}

pub fn dispose_route_opt_map() {
    dispose_route_opt_map_js();
}
