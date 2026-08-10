use leptos::prelude::*;
use wasm_bindgen::prelude::*;

use crate::api::TripPoint;
use crate::units::{instant_economy, use_unit_prefs, UnitSystem};
use crate::components::{Icon, IconColor, IconSize};

#[wasm_bindgen(inline_js = r#"
const __tripCharts = new Map();
const __tripChartTimes = new Map(); // elId -> full ISO timestamps aligned with category axis
let __tripSelection = null; // { iso, dataIndexHint }
let __zoomState = { start: 0, end: 100 };
let __syncingZoom = false;
let __selectionRaf = null;
let __pendingSelection = null;

// Do NOT echarts.connect() all panels — tooltip/axisPointer fan-out was the hover lag.
// Only dataZoom (slider / pan) is shared via dispatchAction below.

function formatNum2(v) {
  if (v == null || v === '' || v === '-') return v;
  if (Array.isArray(v)) {
    return v.map(formatNum2).join(', ');
  }
  const n = typeof v === 'number' ? v : Number(v);
  if (!Number.isFinite(n)) return v;
  return n.toFixed(2);
}

function applyTwoDecimalFormatters(option) {
  if (!option || typeof option !== 'object') return;
  if (!option.tooltip || typeof option.tooltip !== 'object') {
    option.tooltip = {};
  }
  option.tooltip.valueFormatter = (value) => formatNum2(value);

  const axes = option.yAxis;
  const list = Array.isArray(axes) ? axes : axes ? [axes] : [];
  for (const axis of list) {
    if (!axis || typeof axis !== 'object') continue;
    if (!axis.axisLabel || typeof axis.axisLabel !== 'object') {
      axis.axisLabel = {};
    }
    axis.axisLabel.formatter = (value) => formatNum2(value);
  }
}

function parseTimeMs(iso) {
  if (!iso) return null;
  const t = Date.parse(iso);
  return Number.isFinite(t) ? t : null;
}

function nearestTimeIndex(times, iso) {
  if (!times || !times.length) return -1;
  const target = parseTimeMs(iso);
  if (target == null) {
    return times.indexOf(iso);
  }
  let best = 0;
  let bestDist = Infinity;
  for (let i = 0; i < times.length; i++) {
    const t = parseTimeMs(times[i]);
    if (t == null) continue;
    const d = Math.abs(t - target);
    if (d < bestDist) {
      bestDist = d;
      best = i;
    }
  }
  return best;
}

function selectionMarkLine(dataIndex, label, showLabel) {
  return {
    symbol: 'none',
    animation: false,
    silent: true,
    label: showLabel
      ? {
          show: true,
          formatter: label || 'Selected',
          color: '#fdf2f8',
          backgroundColor: 'rgba(190, 24, 93, 0.92)',
          padding: [3, 6],
          borderRadius: 4,
          fontSize: 11
        }
      : { show: false },
    lineStyle: {
      color: '#f472b6',
      width: 2,
      type: 'solid',
      opacity: 0.95
    },
    data: [{ xAxis: dataIndex }]
  };
}

function clearMarkLines(chart) {
  const opt = chart.getOption();
  const series = opt.series || [];
  if (!series.length) return;
  chart.setOption({
    series: series.map(() => ({ markLine: { data: [] } }))
  }, { lazyUpdate: true, silent: true });
}

function applySelectionToChart(elId, chart, iso) {
  const times = __tripChartTimes.get(elId) || [];
  let dataIndex = nearestTimeIndex(times, iso);
  if (dataIndex < 0) {
    const opt = chart.getOption();
    const cats = (opt.xAxis && opt.xAxis[0] && opt.xAxis[0].data) || [];
    if (!cats.length) return -1;
    dataIndex = Math.min(
      __tripSelection && __tripSelection.dataIndexHint != null
        ? __tripSelection.dataIndexHint
        : 0,
      cats.length - 1
    );
  }

  const opt = chart.getOption();
  const series = opt.series || [];
  if (!series.length) return dataIndex;

  const label = (() => {
    const t = times[dataIndex] || iso || '';
    if (t.includes('T')) {
      const part = t.split('T')[1] || t;
      return part.replace(/Z$/, '').split('.')[0];
    }
    return 'Selected';
  })();

  // Mark line on first series only — cheaper than updating every series + no showTip fan-out.
  chart.setOption({
    series: series.map((_, i) => ({
      markLine: i === 0
        ? selectionMarkLine(dataIndex, label, true)
        : { data: [] }
    }))
  }, { lazyUpdate: true, silent: true });

  return dataIndex;
}

function flushTripSelection() {
  __selectionRaf = null;
  const pending = __pendingSelection;
  __pendingSelection = null;
  if (!pending || !pending.iso) return;

  __tripSelection = {
    iso: pending.iso,
    dataIndexHint: pending.dataIndexHint != null ? pending.dataIndexHint : null
  };
  let resolved = null;
  for (const [elId, chart] of __tripCharts.entries()) {
    const idx = applySelectionToChart(elId, chart, pending.iso);
    if (resolved == null && idx >= 0) resolved = idx;
  }
  if (resolved != null) {
    __tripSelection.dataIndexHint = resolved;
  }
  if (!pending.silentEvent) {
    try {
      window.dispatchEvent(new CustomEvent('trip-telemetry-select', {
        detail: { iso: pending.iso, dataIndex: __tripSelection.dataIndexHint }
      }));
    } catch (_) {}
  }
}

function selectTripTelemetryByTime(iso, dataIndexHint, silentEvent) {
  if (!iso) return;
  __pendingSelection = {
    iso,
    dataIndexHint: dataIndexHint != null ? dataIndexHint : null,
    silentEvent: !!silentEvent
  };
  if (__selectionRaf != null) return;
  __selectionRaf = requestAnimationFrame(flushTripSelection);
}

function clearTripTelemetrySelection() {
  __pendingSelection = null;
  if (__selectionRaf != null) {
    cancelAnimationFrame(__selectionRaf);
    __selectionRaf = null;
  }
  __tripSelection = null;
  for (const chart of __tripCharts.values()) {
    clearMarkLines(chart);
  }
  try {
    window.dispatchEvent(new CustomEvent('trip-telemetry-clear'));
  } catch (_) {}
}

function readZoomBatch(params) {
  // datazoom event may carry start/end, or we read from the source chart option.
  if (params && typeof params.start === 'number' && typeof params.end === 'number') {
    return { start: params.start, end: params.end };
  }
  if (params && Array.isArray(params.batch) && params.batch.length) {
    const b = params.batch[0];
    if (typeof b.start === 'number' && typeof b.end === 'number') {
      return { start: b.start, end: b.end };
    }
  }
  return null;
}

function bindChartInteractions(elId, chart) {
  if (chart.__tripBound) return;
  chart.__tripBound = true;

  chart.on('datazoom', (params) => {
    if (__syncingZoom) return;
    let next = readZoomBatch(params);
    if (!next) {
      try {
        const opt = chart.getOption();
        const dz = (opt.dataZoom || [])[0];
        if (dz && typeof dz.start === 'number' && typeof dz.end === 'number') {
          next = { start: dz.start, end: dz.end };
        }
      } catch (_) {}
    }
    if (!next) return;
    if (next.start === __zoomState.start && next.end === __zoomState.end) return;
    __zoomState = next;
    __syncingZoom = true;
    try {
      for (const [id, c] of __tripCharts.entries()) {
        if (id === elId) continue;
        c.dispatchAction({
          type: 'dataZoom',
          start: next.start,
          end: next.end,
          // both inside + slider share the same percent range
          dataZoomIndex: undefined
        });
      }
    } finally {
      __syncingZoom = false;
    }
  });

  chart.on('click', (params) => {
    if (params == null || params.dataIndex == null) return;
    const times = __tripChartTimes.get(elId) || [];
    const iso = times[params.dataIndex];
    if (iso) selectTripTelemetryByTime(iso, params.dataIndex, false);
  });
}

// Bridge for the map (and legend clear control).
window.__tripTelemetry = {
  selectByTime: selectTripTelemetryByTime,
  clearSelection: clearTripTelemetrySelection,
  getSelection: () => __tripSelection
};

export function renderTelemetryChart(elId, optionJson) {
  const el = document.getElementById(elId);
  if (!el || !window.echarts) return;
  let chart = __tripCharts.get(elId);
  if (!chart) {
    chart = echarts.init(el, 'dark', { renderer: 'canvas' });
    __tripCharts.set(elId, chart);
    const onResize = () => {
      const c = __tripCharts.get(elId);
      if (c) c.resize();
    };
    window.addEventListener('resize', onResize);
    chart.__onResize = onResize;
    bindChartInteractions(elId, chart);
  }
  let option;
  try {
    option = typeof optionJson === 'string' ? JSON.parse(optionJson) : optionJson;
  } catch (e) {
    console.error('chart option parse failed', e);
    return;
  }
  // Optional full timestamps for map↔chart sync (stripped before setOption).
  if (Array.isArray(option.__times)) {
    __tripChartTimes.set(elId, option.__times);
    delete option.__times;
  }
  // Preserve shared zoom when re-rendering options.
  if (option.dataZoom && Array.isArray(option.dataZoom)) {
    for (const dz of option.dataZoom) {
      if (dz && typeof dz === 'object') {
        dz.start = __zoomState.start;
        dz.end = __zoomState.end;
      }
    }
  }
  applyTwoDecimalFormatters(option);
  chart.setOption(option, { notMerge: true, lazyUpdate: true });
  requestAnimationFrame(() => {
    chart.resize();
    if (__tripSelection && __tripSelection.iso) {
      applySelectionToChart(elId, chart, __tripSelection.iso);
    }
  });
}

export function disposeTelemetryChart(elId) {
  const chart = __tripCharts.get(elId);
  if (!chart) return;
  if (chart.__onResize) {
    window.removeEventListener('resize', chart.__onResize);
  }
  chart.dispose();
  __tripCharts.delete(elId);
  __tripChartTimes.delete(elId);
}
"#)]
extern "C" {
    fn renderTelemetryChart(el_id: &str, option_json: &str);
    fn disposeTelemetryChart(el_id: &str);
}

#[derive(Clone, PartialEq)]
struct ChartSeriesSpec {
    name: String,
    data: Vec<Option<f64>>,
    y_axis_index: i32,
    area: bool,
}

/// Panel render mode. Mixture uses candlesticks so trim oscillation bands are visible.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PanelKind {
    Lines,
    /// STFT/LTFT as OHLC candles + optional lambda line; healthy ±10% band on % axis.
    MixtureCandles,
}

#[derive(Clone, PartialEq)]
struct PanelDef {
    id: &'static str,
    title: &'static str,
    blurb: &'static str,
    primary: bool,
    y_left: String,
    y_right: Option<String>,
    series: Vec<ChartSeriesSpec>,
    kind: PanelKind,
}

/// Healthy closed-loop fuel trim band (percent). Outside this is worth investigating.
const FUEL_TRIM_HEALTHY_PCT: f64 = 10.0;

/// Target candle count for mixture panel (bucket STFT/LTFT into OHLC windows).
const MIXTURE_CANDLE_TARGET: usize = 160;

/// EMA span (~samples) for Smooth mode on line series after downsample.
/// Large on purpose: default charts are trend-first; Raw keeps full jagged detail.
const SMOOTH_SPAN: f64 = 28.0;

/// Second EMA pass span (slightly shorter) for a butter-smooth trend without
/// adding another control. Combined with SMOOTH_SPAN this is much calmer than a
/// single light EMA.
const SMOOTH_SPAN_PASS2: f64 = 18.0;

/// ECharts bezier factor when Smooth is on (0 = polyline).
const LINE_SMOOTH_VISUAL: f64 = 0.62;

const CATEGORY_STORAGE_KEY: &str = "trip-tel-category";

/// Filter tabs for progressive disclosure (analytics-style, not accordion walls).
const CATEGORY_TABS: &[(&str, &str)] = &[
    ("overview", "Overview"),
    ("drive", "Drive"),
    ("engine", "Engine"),
    ("fuel", "Fuel"),
    ("thermal", "Thermal"),
    ("all", "All"),
];

fn series_has_data(data: &[Option<f64>]) -> bool {
    data.iter().any(|v| v.is_some())
}

/// Round chart values to 2 decimal places for display consistency.
fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn round_series_2dp(data: &[Option<f64>]) -> Vec<Option<f64>> {
    data.iter().map(|v| v.map(round2)).collect()
}

fn downsample_points(points: &[TripPoint], max_n: usize) -> Vec<TripPoint> {
    if points.len() <= max_n || max_n < 3 {
        return points.to_vec();
    }
    let last = points.len() - 1;
    let mut out = Vec::with_capacity(max_n);
    for i in 0..max_n {
        let idx = if i == max_n - 1 {
            last
        } else {
            (i * last) / (max_n - 1)
        };
        out.push(points[idx].clone());
    }
    out
}

/// Null-aware EMA: gaps stay gaps and restart the filter after nulls.
fn ema_series(data: &[Option<f64>], span: f64) -> Vec<Option<f64>> {
    let alpha = 2.0 / (span.max(1.0) + 1.0);
    let mut out = Vec::with_capacity(data.len());
    let mut prev: Option<f64> = None;
    for v in data {
        match (*v, prev) {
            (None, _) => {
                out.push(None);
                prev = None;
            }
            (Some(x), None) => {
                out.push(Some(x));
                prev = Some(x);
            }
            (Some(x), Some(p)) => {
                let y = alpha * x + (1.0 - alpha) * p;
                out.push(Some(y));
                prev = Some(y);
            }
        }
    }
    out
}

fn maybe_smooth_series(data: &[Option<f64>], smooth: bool) -> Vec<Option<f64>> {
    if smooth {
        // Two-pass null-aware EMA: first kills high-frequency OBD noise, second
        // settles the trend. Raw mode skips this entirely.
        let once = ema_series(data, SMOOTH_SPAN);
        ema_series(&once, SMOOTH_SPAN_PASS2)
    } else {
        data.to_vec()
    }
}

fn mean_finite(vals: impl Iterator<Item = f64>) -> Option<f64> {
    let mut sum = 0.0;
    let mut n = 0usize;
    for v in vals {
        if v.is_finite() {
            sum += v;
            n += 1;
        }
    }
    if n == 0 {
        None
    } else {
        Some(sum / n as f64)
    }
}

fn max_finite(vals: impl Iterator<Item = f64>) -> Option<f64> {
    let mut m: Option<f64> = None;
    for v in vals {
        if !v.is_finite() {
            continue;
        }
        m = Some(match m {
            Some(cur) if cur >= v => cur,
            _ => v,
        });
    }
    m
}

fn min_finite(vals: impl Iterator<Item = f64>) -> Option<f64> {
    let mut m: Option<f64> = None;
    for v in vals {
        if !v.is_finite() {
            continue;
        }
        m = Some(match m {
            Some(cur) if cur <= v => cur,
            _ => v,
        });
    }
    m
}

fn last_finite(vals: impl Iterator<Item = f64>) -> Option<f64> {
    let mut last = None;
    for v in vals {
        if v.is_finite() {
            last = Some(v);
        }
    }
    last
}

fn load_category_filter() -> String {
    let default = "overview".to_string();
    let Some(win) = web_sys::window() else {
        return default;
    };
    let Ok(Some(storage)) = win.session_storage() else {
        return default;
    };
    match storage.get_item(CATEGORY_STORAGE_KEY) {
        Ok(Some(raw)) => {
            let key = raw.trim().to_ascii_lowercase();
            if CATEGORY_TABS.iter().any(|(k, _)| *k == key) {
                key
            } else {
                default
            }
        }
        _ => default,
    }
}

fn save_category_filter(key: &str) {
    let Some(win) = web_sys::window() else {
        return;
    };
    let Ok(Some(storage)) = win.session_storage() else {
        return;
    };
    let _ = storage.set_item(CATEGORY_STORAGE_KEY, key);
}

fn time_labels(points: &[TripPoint]) -> Vec<String> {
    points
        .iter()
        .map(|p| {
            // ISO-ish timestamps → show time portion when present
            let s = p.recorded_at.as_str();
            if let Some(t) = s.split('T').nth(1) {
                t.trim_end_matches('Z')
                    .split('.')
                    .next()
                    .unwrap_or(t)
                    .chars()
                    .take(8)
                    .collect()
            } else if s.len() >= 19 {
                s[11..19].to_string()
            } else {
                s.to_string()
            }
        })
        .collect()
}

fn coalesce_speed(p: &TripPoint) -> Option<f64> {
    p.vehicle_speed_kph.or(p.engine_vel)
}

fn coalesce_rpm(p: &TripPoint) -> Option<f64> {
    p.vehicle_engine_rpm.or(p.engine_rpm)
}

fn instant_economy_point(speed: Option<f64>, fuel_rate: Option<f64>, system: UnitSystem) -> Option<f64> {
    instant_economy(speed, fuel_rate, system)
}

/// Bucket a line series into ECharts candlestick OHLC: `[open, close, low, high]`.
fn series_to_ohlc(data: &[Option<f64>], bucket: usize) -> Vec<serde_json::Value> {
    let bucket = bucket.max(1);
    let mut out = Vec::with_capacity(data.len().div_ceil(bucket));
    for chunk in data.chunks(bucket) {
        let vals: Vec<f64> = chunk.iter().filter_map(|v| *v).collect();
        if vals.is_empty() {
            out.push(serde_json::Value::Null);
            continue;
        }
        let open = round2(vals[0]);
        let close = round2(*vals.last().unwrap());
        let mut low = vals[0];
        let mut high = vals[0];
        for v in &vals[1..] {
            if *v < low {
                low = *v;
            }
            if *v > high {
                high = *v;
            }
        }
        out.push(serde_json::json!([open, close, round2(low), round2(high)]));
    }
    out
}

fn bucket_labels(labels: &[String], bucket: usize) -> Vec<String> {
    let bucket = bucket.max(1);
    labels
        .chunks(bucket)
        .map(|c| c.last().cloned().unwrap_or_default())
        .collect()
}

fn bucket_times(times: &[String], bucket: usize) -> Vec<String> {
    let bucket = bucket.max(1);
    times
        .chunks(bucket)
        .map(|c| c.last().cloned().unwrap_or_default())
        .collect()
}

fn mixture_bucket_size(n: usize) -> usize {
    if n <= MIXTURE_CANDLE_TARGET {
        1
    } else {
        (n + MIXTURE_CANDLE_TARGET - 1) / MIXTURE_CANDLE_TARGET
    }
}

fn shared_chart_chrome(
    use_right: bool,
    boundary_gap: bool,
) -> (
    serde_json::Value,
    serde_json::Value,
    serde_json::Value,
    Vec<serde_json::Value>,
) {
    // Title lives in HTML panel chrome — keep plot area for data.
    let tooltip = serde_json::json!({
        "trigger": "axis",
        "axisPointer": {
            "type": "line",
            "snap": true,
            "lineStyle": { "color": "rgba(148,163,184,0.65)", "width": 1 },
            "label": { "show": false }
        },
        "backgroundColor": "rgba(18,26,43,0.95)",
        "borderColor": "#24314a",
        "textStyle": { "color": "#e8eefc", "fontSize": 12 }
    });
    let legend = serde_json::json!({
        "top": 6,
        "right": 8,
        "left": 8,
        "orient": "horizontal",
        "align": "left",
        "itemGap": 12,
        "itemWidth": 14,
        "itemHeight": 8,
        "padding": [2, 4, 2, 4],
        "textStyle": { "color": "#93a0b8", "fontSize": 11 },
        "type": "scroll",
        "pageIconColor": "#93a0b8",
        "pageTextStyle": { "color": "#93a0b8" }
    });
    let grid = serde_json::json!({
        "left": 62,
        "right": if use_right { 62 } else { 28 },
        "top": 42,
        "bottom": 58,
        "containLabel": false
    });
    // Wheel zoom disabled (user request). Slider + drag-pan still work; zoom range is
    // shared across panels in JS without echarts.connect tooltips.
    let data_zoom = vec![
        serde_json::json!({
            "type": "inside",
            "xAxisIndex": 0,
            "start": 0,
            "end": 100,
            "zoomOnMouseWheel": false,
            "moveOnMouseWheel": false,
            "moveOnMouseMove": true,
            "preventDefaultMouseMove": false
        }),
        serde_json::json!({
            "type": "slider",
            "xAxisIndex": 0,
            "height": 18,
            "bottom": 8,
            "borderColor": "#24314a",
            "fillerColor": "rgba(59,130,246,0.25)",
            "handleStyle": { "color": "#3b82f6" },
            "textStyle": { "color": "#93a0b8" },
            "dataBackground": {
                "lineStyle": { "color": "#3b82f6" },
                "areaStyle": { "color": "rgba(59,130,246,0.15)" }
            }
        }),
    ];
    let _ = boundary_gap;
    (tooltip, legend, grid, data_zoom)
}

/// Color for a Y-axis from the first series bound to that axis (palette index).
/// Left axis falls back to the first series when everything shares one axis;
/// otherwise muted slate if nothing is bound.
fn y_axis_series_color(series: &[ChartSeriesSpec], colors: &[&str], axis_idx: i32) -> String {
    let by_axis = series
        .iter()
        .enumerate()
        .find(|(_, s)| s.y_axis_index == axis_idx);
    let pick = if axis_idx == 0 {
        by_axis.or_else(|| series.iter().enumerate().next())
    } else {
        by_axis
    };
    pick.map(|(i, _)| colors[i % colors.len()].to_string())
        .unwrap_or_else(|| "#93a0b8".to_string())
}

fn build_option(
    labels: &[String],
    times: &[String],
    series: &[ChartSeriesSpec],
    y_left_name: &str,
    y_right_name: Option<&str>,
    line_smooth: f64,
) -> serde_json::Value {
    let colors = [
        "#3b82f6", "#22c55e", "#f59e0b", "#a78bfa", "#22d3ee", "#f472b6", "#eab308",
    ];
    let use_right = y_right_name.is_some() && series.iter().any(|s| s.y_axis_index == 1);
    let (tooltip, legend, grid, data_zoom) = shared_chart_chrome(use_right, false);

    // Match axis chrome (name, ticks, line) to the primary series on that axis so
    // dual-axis panels are readable at a glance without hunting the legend.
    let left_color = y_axis_series_color(series, &colors, 0);
    let right_color = y_axis_series_color(series, &colors, 1);

    // Axis unit names sit mid-axis so they never collide with clickable legend items.
    let mut y_axis = vec![serde_json::json!({
        "type": "value",
        "name": y_left_name,
        "nameLocation": "middle",
        "nameGap": 46,
        "nameRotate": 90,
        "nameTextStyle": { "color": left_color, "fontSize": 11, "padding": [0, 0, 0, 0] },
        "splitLine": { "lineStyle": { "color": "rgba(36,49,74,0.85)" } },
        "axisLabel": { "color": left_color, "hideOverlap": true },
        "axisLine": { "show": true, "lineStyle": { "color": left_color } },
        "axisTick": { "lineStyle": { "color": left_color } },
        "scale": true
    })];
    if use_right {
        y_axis.push(serde_json::json!({
            "type": "value",
            "name": y_right_name.unwrap_or(""),
            "nameLocation": "middle",
            "nameGap": 46,
            "nameRotate": 90,
            "nameTextStyle": { "color": right_color, "fontSize": 11 },
            "splitLine": { "show": false },
            "axisLabel": { "color": right_color, "hideOverlap": true },
            "axisLine": { "show": true, "lineStyle": { "color": right_color } },
            "axisTick": { "lineStyle": { "color": right_color } },
            "scale": true
        }));
    }

    let series_json: Vec<serde_json::Value> = series
        .iter()
        .enumerate()
        .map(|(idx, s)| {
            let color = colors[idx % colors.len()];
            let mut obj = serde_json::json!({
                "name": s.name,
                "type": "line",
                "smooth": line_smooth,
                "showSymbol": false,
                "connectNulls": false,
                "animation": false,
                "yAxisIndex": if use_right { s.y_axis_index } else { 0 },
                "data": round_series_2dp(&s.data),
                "lineStyle": { "width": 2, "color": color },
                "itemStyle": { "color": color }
            });
            if s.area {
                obj.as_object_mut().unwrap().insert(
                    "areaStyle".into(),
                    serde_json::json!({
                        "color": {
                            "type": "linear",
                            "x": 0, "y": 0, "x2": 0, "y2": 1,
                            "colorStops": [
                                { "offset": 0, "color": format!("{color}55") },
                                { "offset": 1, "color": format!("{color}05") }
                            ]
                        }
                    }),
                );
            }
            obj
        })
        .collect();

    serde_json::json!({
        "backgroundColor": "transparent",
        "animation": false,
        "color": colors,
        "tooltip": tooltip,
        "legend": legend,
        "grid": grid,
        "dataZoom": data_zoom,
        "xAxis": {
            "type": "category",
            "data": labels,
            "boundaryGap": false,
            "axisPointer": { "show": true },
            "axisLabel": { "color": "#93a0b8", "hideOverlap": true, "fontSize": 10 },
            "axisLine": { "lineStyle": { "color": "#24314a" } },
            "axisTick": { "show": false }
        },
        "yAxis": y_axis,
        "series": series_json,
        // Stripped in JS before setOption; used for map↔chart time sync.
        "__times": times
    })
}

/// Mixture & trims: STFT/LTFT as time-bucketed candlesticks + lambda line.
/// Green band marks the healthy ±10% trim window.
fn build_mixture_option(
    labels: &[String],
    times: &[String],
    series: &[ChartSeriesSpec],
    line_smooth: f64,
) -> serde_json::Value {
    let bucket = mixture_bucket_size(labels.len().max(times.len()));
    let x_labels = bucket_labels(labels, bucket);
    let x_times = bucket_times(times, bucket);

    let stft = series.iter().find(|s| s.name.starts_with("STFT"));
    let ltft = series.iter().find(|s| s.name.starts_with("LTFT"));
    let lambda = series.iter().find(|s| s.name.to_ascii_lowercase().contains("lambda"));

    let use_right = lambda.map(|s| series_has_data(&s.data)).unwrap_or(false);
    let (_tooltip, legend, grid, data_zoom) = shared_chart_chrome(use_right, true);

    // Match axis chrome to the primary series on each side (STFT green / lambda violet).
    const TRIM_AXIS_COLOR: &str = "#22c55e";
    const LAMBDA_AXIS_COLOR: &str = "#a78bfa";
    let band = FUEL_TRIM_HEALTHY_PCT;
    let y_axis = {
        let mut axes = vec![serde_json::json!({
            "type": "value",
            "name": "% trim",
            "nameLocation": "middle",
            "nameGap": 46,
            "nameRotate": 90,
            "nameTextStyle": { "color": TRIM_AXIS_COLOR, "fontSize": 11 },
            "splitLine": { "lineStyle": { "color": "rgba(36,49,74,0.85)" } },
            "axisLabel": { "color": TRIM_AXIS_COLOR, "hideOverlap": true },
            "axisLine": { "show": true, "lineStyle": { "color": TRIM_AXIS_COLOR } },
            "axisTick": { "lineStyle": { "color": TRIM_AXIS_COLOR } },
            "scale": true,
            "min": serde_json::Value::Null,
            "max": serde_json::Value::Null
        })];
        if use_right {
            axes.push(serde_json::json!({
                "type": "value",
                "name": "λ",
                "nameLocation": "middle",
                "nameGap": 46,
                "nameRotate": 90,
                "nameTextStyle": { "color": LAMBDA_AXIS_COLOR, "fontSize": 11 },
                "splitLine": { "show": false },
                "axisLabel": { "color": LAMBDA_AXIS_COLOR, "hideOverlap": true },
                "axisLine": { "show": true, "lineStyle": { "color": LAMBDA_AXIS_COLOR } },
                "axisTick": { "lineStyle": { "color": LAMBDA_AXIS_COLOR } },
                "scale": true
            }));
        }
        axes
    };

    let healthy_mark_area = serde_json::json!({
        "silent": true,
        "animation": false,
        "itemStyle": { "color": "rgba(34, 197, 94, 0.10)" },
        "data": [[
            { "yAxis": -band },
            { "yAxis": band }
        ]]
    });
    let healthy_mark_line = serde_json::json!({
        "silent": true,
        "animation": false,
        "symbol": "none",
        "label": {
            "show": true,
            "formatter": "healthy ±10%",
            "color": "#86efac",
            "fontSize": 10,
            "position": "insideEndTop"
        },
        "lineStyle": { "color": "rgba(34, 197, 94, 0.55)", "type": "dashed", "width": 1 },
        "data": [
            { "yAxis": band },
            { "yAxis": -band }
        ]
    });

    let mut series_json: Vec<serde_json::Value> = Vec::new();

    if let Some(s) = stft {
        if series_has_data(&s.data) {
            series_json.push(serde_json::json!({
                "name": "STFT (%)",
                "type": "candlestick",
                "yAxisIndex": 0,
                "animation": false,
                "barMaxWidth": 10,
                "itemStyle": {
                    "color": "#22c55e",
                    "color0": "#ef4444",
                    "borderColor": "#16a34a",
                    "borderColor0": "#dc2626"
                },
                "data": series_to_ohlc(&s.data, bucket),
                "markArea": healthy_mark_area,
                "markLine": healthy_mark_line
            }));
        }
    }

    if let Some(s) = ltft {
        if series_has_data(&s.data) {
            // LTFT usually moves slowly — still show as candles so high/low of the
            // window is obvious when learned trim drifts outside healthy band.
            series_json.push(serde_json::json!({
                "name": "LTFT (%)",
                "type": "candlestick",
                "yAxisIndex": 0,
                "animation": false,
                "barMaxWidth": 10,
                "itemStyle": {
                    "color": "#38bdf8",
                    "color0": "#f97316",
                    "borderColor": "#0ea5e9",
                    "borderColor0": "#ea580c"
                },
                "data": series_to_ohlc(&s.data, bucket)
            }));
        }
    }

    if let Some(s) = lambda {
        if series_has_data(&s.data) {
            let lambda_data: Vec<serde_json::Value> = s
                .data
                .chunks(bucket.max(1))
                .map(|chunk| {
                    // last non-null in bucket
                    chunk
                        .iter()
                        .rev()
                        .find_map(|v| *v)
                        .map(|v| serde_json::json!(round2(v)))
                        .unwrap_or(serde_json::Value::Null)
                })
                .collect();
            series_json.push(serde_json::json!({
                "name": "Lambda cmd",
                "type": "line",
                "yAxisIndex": if use_right { 1 } else { 0 },
                "smooth": line_smooth,
                "showSymbol": false,
                "connectNulls": false,
                "animation": false,
                "data": lambda_data,
                "lineStyle": { "width": 2, "color": "#a78bfa" },
                "itemStyle": { "color": "#a78bfa" }
            }));
        }
    }

    serde_json::json!({
        "backgroundColor": "transparent",
        "animation": false,
        "color": ["#22c55e", "#38bdf8", "#a78bfa"],
        "tooltip": {
            "trigger": "axis",
            "axisPointer": {
                "type": "shadow",
                "snap": true,
                "shadowStyle": { "color": "rgba(148,163,184,0.12)" },
                "label": { "show": false }
            },
            "backgroundColor": "rgba(18,26,43,0.95)",
            "borderColor": "#24314a",
            "textStyle": { "color": "#e8eefc", "fontSize": 12 }
        },
        "legend": legend,
        "grid": grid,
        "dataZoom": data_zoom,
        "xAxis": {
            "type": "category",
            "data": x_labels,
            // Candles need a gap between categories.
            "boundaryGap": true,
            "axisPointer": { "show": true },
            "axisLabel": { "color": "#93a0b8", "hideOverlap": true, "fontSize": 10 },
            "axisLine": { "lineStyle": { "color": "#24314a" } },
            "axisTick": { "show": false }
        },
        "yAxis": y_axis,
        "series": series_json,
        "__times": x_times
    })
}

#[component]
fn TelemetryChart(
    chart_id: String,
    labels: Vec<String>,
    times: Vec<String>,
    series: Vec<ChartSeriesSpec>,
    y_left_name: String,
    y_right_name: Option<String>,
    kind: PanelKind,
    line_smooth: f64,
) -> impl IntoView {
    let el_id = chart_id.clone();
    let el_id_dispose = chart_id.clone();
    let labels_c = labels.clone();
    let times_c = times.clone();
    let series_c = series.clone();
    let y_left_c = y_left_name.clone();
    let y_right_c = y_right_name.clone();

    Effect::new(move |_| {
        if series_c.is_empty() || labels_c.is_empty() {
            return;
        }
        let option = match kind {
            PanelKind::MixtureCandles => {
                build_mixture_option(&labels_c, &times_c, &series_c, line_smooth)
            }
            PanelKind::Lines => build_option(
                &labels_c,
                &times_c,
                &series_c,
                &y_left_c,
                y_right_c.as_deref(),
                line_smooth,
            ),
        };
        if let Ok(s) = serde_json::to_string(&option) {
            renderTelemetryChart(&el_id, &s);
        }
    });

    on_cleanup(move || {
        disposeTelemetryChart(&el_id_dispose);
    });

    view! { <div id=chart_id class="chart chart-tall"></div> }
}

fn filter_panels(mut panels: Vec<PanelDef>) -> Vec<PanelDef> {
    panels.retain(|p| p.series.iter().any(|s| series_has_data(&s.data)));
    for p in &mut panels {
        p.series.retain(|s| series_has_data(&s.data));
    }
    panels
}

fn apply_smooth_to_panels(panels: &mut [PanelDef], smooth: bool) {
    for p in panels.iter_mut() {
        match p.kind {
            PanelKind::Lines => {
                for s in p.series.iter_mut() {
                    s.data = maybe_smooth_series(&s.data, smooth);
                }
            }
            PanelKind::MixtureCandles => {
                // Candles stay bucketed; only the lambda line follows Smooth/Raw.
                for s in p.series.iter_mut() {
                    if s.name.to_ascii_lowercase().contains("lambda") {
                        s.data = maybe_smooth_series(&s.data, smooth);
                    }
                }
            }
        }
    }
}

#[derive(Clone, PartialEq)]
struct SignalChip {
    label: String,
    value: String,
}

fn build_signal_chips(raw: &[TripPoint], prefs: &crate::units::UnitPrefs) -> Vec<SignalChip> {
    let mut chips = Vec::new();
    let speeds: Vec<f64> = raw.iter().filter_map(coalesce_speed).collect();
    if let (Some(avg), Some(max)) = (
        mean_finite(speeds.iter().copied()),
        max_finite(speeds.iter().copied()),
    ) {
        chips.push(SignalChip {
            label: "Speed".into(),
            value: format!(
                "avg {:.0} · max {:.0} {}",
                avg, max, prefs.labels.speed
            ),
        });
    }
    if let Some(peak) = max_finite(raw.iter().filter_map(coalesce_rpm)) {
        chips.push(SignalChip {
            label: "RPM".into(),
            value: format!("peak {:.0}", peak),
        });
    }
    if let Some(avg_rate) = mean_finite(raw.iter().filter_map(|p| p.fuel_consumption_rate)) {
        chips.push(SignalChip {
            label: "Fuel rate".into(),
            value: format!("{avg_rate:.2} {}", prefs.labels.fuel_rate),
        });
    }
    let eco_vals: Vec<f64> = raw
        .iter()
        .filter_map(|p| {
            instant_economy_point(coalesce_speed(p), p.fuel_consumption_rate, prefs.system)
        })
        .collect();
    if let Some(avg_eco) = mean_finite(eco_vals.into_iter()) {
        chips.push(SignalChip {
            label: "Economy".into(),
            value: format!("{avg_eco:.1} {}", prefs.labels.fuel_economy),
        });
    }
    let coolants: Vec<f64> = raw.iter().filter_map(|p| p.engine_coolant_temp_c).collect();
    if let (Some(last), Some(max)) = (
        last_finite(coolants.iter().copied()),
        max_finite(coolants.iter().copied()),
    ) {
        chips.push(SignalChip {
            label: "Coolant".into(),
            value: format!("last {:.0}° · max {:.0}°C", last, max),
        });
    }
    let volts: Vec<f64> = raw.iter().filter_map(|p| p.control_module_voltage).collect();
    if let (Some(min_v), Some(max_v)) = (
        min_finite(volts.iter().copied()),
        max_finite(volts.iter().copied()),
    ) {
        chips.push(SignalChip {
            label: "Voltage".into(),
            value: format!("{min_v:.1}–{max_v:.1} V"),
        });
    }
    chips
}

/// Sectioned trip telemetry charts built from full OBD point payloads.
#[component]
pub fn TripTelemetryDashboard(points: Signal<Vec<TripPoint>>) -> impl IntoView {
    let prefs = use_unit_prefs();
    let smooth = RwSignal::new(true);
    let category = RwSignal::new(load_category_filter());

    let chips = Memo::new(move |_| {
        let unit_prefs = prefs.get();
        let raw = points.get();
        if raw.is_empty() {
            return Vec::new();
        }
        build_signal_chips(&raw, &unit_prefs)
    });

    let model = Memo::new(move |_| {
        let unit_prefs = prefs.get();
        let system = unit_prefs.system;
        let ul = &unit_prefs.labels;
        let want_smooth = smooth.get();
        let raw = points.get();
        if raw.is_empty() {
            return None;
        }
        let pts = downsample_points(&raw, 1200);
        let labels = time_labels(&pts);
        let times: Vec<String> = pts.iter().map(|p| p.recorded_at.clone()).collect();

        let speed: Vec<_> = pts.iter().map(coalesce_speed).collect();
        let rpm: Vec<_> = pts.iter().map(coalesce_rpm).collect();
        let pedal: Vec<_> = pts.iter().map(|p| p.accelerator_pedal_pct).collect();
        let gps_acc: Vec<_> = pts
            .iter()
            .map(|p| {
                if p.gps_acc_m >= 0.0 {
                    Some(p.gps_acc_m)
                } else {
                    None
                }
            })
            .collect();
        let load: Vec<_> = pts.iter().map(|p| p.engine_load_pct).collect();
        let abs_load: Vec<_> = pts.iter().map(|p| p.absolute_engine_load_pct).collect();
        let maf: Vec<_> = pts.iter().map(|p| p.mass_air_flow).collect();
        let map_kpa: Vec<_> = pts
            .iter()
            .map(|p| p.manifold_absolute_pressure_kpa)
            .collect();
        let fuel_rate: Vec<_> = pts.iter().map(|p| p.fuel_consumption_rate).collect();
        let fuel_level: Vec<_> = pts.iter().map(|p| p.fuel_level_pct).collect();
        let stft: Vec<_> = pts.iter().map(|p| p.short_term_fuel_trim_pct).collect();
        let ltft: Vec<_> = pts.iter().map(|p| p.long_term_fuel_trim_pct).collect();
        let lambda: Vec<_> = pts.iter().map(|p| p.lambda_cmd).collect();
        let economy: Vec<_> = pts
            .iter()
            .map(|p| instant_economy_point(coalesce_speed(p), p.fuel_consumption_rate, system))
            .collect();
        let coolant: Vec<_> = pts.iter().map(|p| p.engine_coolant_temp_c).collect();
        let iat: Vec<_> = pts.iter().map(|p| p.intake_air_temperature).collect();
        let ambient: Vec<_> = pts.iter().map(|p| p.ambient_air_temp_c).collect();
        let voltage: Vec<_> = pts.iter().map(|p| p.control_module_voltage).collect();
        let atm: Vec<_> = pts.iter().map(|p| p.atmospheric_pressure).collect();

        let mut sections: Vec<(
            &'static str,
            &'static str,
            &'static str,
            &'static str,
            Vec<PanelDef>,
        )> = Vec::new();

        let mut drive = filter_panels(vec![
            PanelDef {
                id: "drive-speed",
                title: "Speed & pedal",
                blurb: "Road speed versus how hard you pressed the accelerator — demand and pace without opening the hood.",
                primary: true,
                y_left: ul.speed.to_string(),
                y_right: Some("%".to_string()),
                kind: PanelKind::Lines,
                series: vec![
                    ChartSeriesSpec {
                        name: format!("Speed ({})", ul.speed),
                        data: speed.clone(),
                        y_axis_index: 0,
                        area: true,
                    },
                    ChartSeriesSpec {
                        name: "Accelerator (%)".to_string(),
                        data: pedal,
                        y_axis_index: 1,
                        area: false,
                    },
                ],
            },
            PanelDef {
                id: "drive-gps",
                title: "GPS accuracy",
                blurb: "Position uncertainty in meters. Spikes explain map wiggles or unreliable GPS-derived speed.",
                primary: false,
                y_left: "m".to_string(),
                y_right: None,
                kind: PanelKind::Lines,
                series: vec![ChartSeriesSpec {
                    name: "GPS accuracy (m)".to_string(),
                    data: gps_acc,
                    y_axis_index: 0,
                    area: true,
                }],
            },
        ]);
        apply_smooth_to_panels(&mut drive, want_smooth);
        if !drive.is_empty() {
            sections.push((
                "Drive dynamics",
                "path",
                "drive",
                "Pace, pedal, and positioning quality.",
                drive,
            ));
        }

        let mut engine = filter_panels(vec![
            PanelDef {
                id: "engine-rpm-load",
                title: "RPM & load",
                blurb: "Engine speed plus calculated and absolute load — how hard the engine worked and in which gear-ish range.",
                primary: true,
                y_left: "RPM".to_string(),
                y_right: Some("%".to_string()),
                kind: PanelKind::Lines,
                series: vec![
                    ChartSeriesSpec {
                        name: "RPM".to_string(),
                        data: rpm,
                        y_axis_index: 0,
                        area: true,
                    },
                    ChartSeriesSpec {
                        name: "Engine load (%)".to_string(),
                        data: load,
                        y_axis_index: 1,
                        area: false,
                    },
                    ChartSeriesSpec {
                        name: "Absolute load (%)".to_string(),
                        data: abs_load,
                        y_axis_index: 1,
                        area: false,
                    },
                ],
            },
            PanelDef {
                id: "engine-air",
                title: "Airflow & MAP",
                blurb: "Mass air flow and manifold absolute pressure — how the engine was breathing (vacuum, load, or boost).",
                primary: false,
                y_left: "g/s".to_string(),
                y_right: Some("kPa".to_string()),
                kind: PanelKind::Lines,
                series: vec![
                    ChartSeriesSpec {
                        name: "MAF (g/s)".to_string(),
                        data: maf,
                        y_axis_index: 0,
                        area: true,
                    },
                    ChartSeriesSpec {
                        name: "MAP (kPa)".to_string(),
                        data: map_kpa.clone(),
                        y_axis_index: 1,
                        area: false,
                    },
                ],
            },
        ]);
        apply_smooth_to_panels(&mut engine, want_smooth);
        if !engine.is_empty() {
            sections.push((
                "Engine",
                "cpu",
                "engine",
                "Rotation, load, and air intake.",
                engine,
            ));
        }

        let mut fuel = filter_panels(vec![
            PanelDef {
                id: "fuel-rate",
                title: "Fuel rate & economy",
                blurb: "Instant burn rate and efficiency. Economy is noisy at idle or crawl speeds — Smooth mode helps the trend.",
                primary: true,
                y_left: ul.fuel_rate.to_string(),
                y_right: Some(ul.fuel_economy.to_string()),
                kind: PanelKind::Lines,
                series: vec![
                    ChartSeriesSpec {
                        name: format!("Fuel rate ({})", ul.fuel_rate),
                        data: fuel_rate,
                        y_axis_index: 0,
                        area: true,
                    },
                    ChartSeriesSpec {
                        name: format!("Instant {}", ul.fuel_economy),
                        data: economy,
                        y_axis_index: 1,
                        area: false,
                    },
                ],
            },
            PanelDef {
                id: "fuel-mixture",
                title: "Mixture & trims",
                blurb: "Short- and long-term fuel trims plus commanded lambda. Green band marks a healthy closed-loop ±10% window.",
                primary: true,
                y_left: "%".to_string(),
                y_right: Some("λ".to_string()),
                kind: PanelKind::MixtureCandles,
                series: vec![
                    ChartSeriesSpec {
                        name: "STFT (%)".to_string(),
                        data: stft,
                        y_axis_index: 0,
                        area: false,
                    },
                    ChartSeriesSpec {
                        name: "LTFT (%)".to_string(),
                        data: ltft,
                        y_axis_index: 0,
                        area: false,
                    },
                    ChartSeriesSpec {
                        name: "Lambda cmd".to_string(),
                        data: lambda,
                        y_axis_index: 1,
                        area: true,
                    },
                ],
            },
            PanelDef {
                id: "fuel-level",
                title: "Fuel level",
                blurb: "Tank percentage over the trip. Expect slow drift plus step noise from the sender.",
                primary: false,
                y_left: "%".to_string(),
                y_right: None,
                kind: PanelKind::Lines,
                series: vec![ChartSeriesSpec {
                    name: "Fuel level (%)".to_string(),
                    data: fuel_level,
                    y_axis_index: 0,
                    area: true,
                }],
            },
        ]);
        apply_smooth_to_panels(&mut fuel, want_smooth);
        if !fuel.is_empty() {
            sections.push((
                "Fuel & mixture",
                "drop",
                "fuel",
                "Burn rate, tank level, and closed-loop mixture.",
                fuel,
            ));
        }

        let mut thermal = filter_panels(vec![
            PanelDef {
                id: "thermal-temps",
                title: "Temperatures",
                blurb: "Coolant warm-up and overheat risk; intake and ambient air for density and heat soak context.",
                primary: false,
                y_left: "°C".to_string(),
                y_right: None,
                kind: PanelKind::Lines,
                series: vec![
                    ChartSeriesSpec {
                        name: "Coolant (°C)".to_string(),
                        data: coolant,
                        y_axis_index: 0,
                        area: true,
                    },
                    ChartSeriesSpec {
                        name: "Intake air (°C)".to_string(),
                        data: iat,
                        y_axis_index: 0,
                        area: false,
                    },
                    ChartSeriesSpec {
                        name: "Ambient (°C)".to_string(),
                        data: ambient,
                        y_axis_index: 0,
                        area: false,
                    },
                ],
            },
            PanelDef {
                id: "thermal-elec",
                title: "Electrical & pressure",
                blurb: "Control-module voltage (charging health) with manifold and atmospheric pressure for altitude/load context.",
                primary: false,
                y_left: "V".to_string(),
                y_right: Some("kPa".to_string()),
                kind: PanelKind::Lines,
                series: vec![
                    ChartSeriesSpec {
                        name: "Module voltage (V)".to_string(),
                        data: voltage,
                        y_axis_index: 0,
                        area: true,
                    },
                    ChartSeriesSpec {
                        name: "MAP (kPa)".to_string(),
                        data: map_kpa,
                        y_axis_index: 1,
                        area: false,
                    },
                    ChartSeriesSpec {
                        name: "Atmospheric (kPa)".to_string(),
                        data: atm,
                        y_axis_index: 1,
                        area: false,
                    },
                ],
            },
        ]);
        apply_smooth_to_panels(&mut thermal, want_smooth);
        if !thermal.is_empty() {
            sections.push((
                "Thermal & electrical",
                "lightning",
                "thermal",
                "Heat and electrical health.",
                thermal,
            ));
        }

        let has_obd = raw.iter().any(|p| {
            coalesce_speed(p).is_some()
                || coalesce_rpm(p).is_some()
                || p.fuel_consumption_rate.is_some()
                || p.engine_load_pct.is_some()
                || p.absolute_engine_load_pct.is_some()
                || p.short_term_fuel_trim_pct.is_some()
                || p.long_term_fuel_trim_pct.is_some()
                || p.fuel_level_pct.is_some()
                || p.accelerator_pedal_pct.is_some()
                || p.ambient_air_temp_c.is_some()
                || p.odometer_value_km.is_some()
                || p.engine_coolant_temp_c.is_some()
                || p.manifold_absolute_pressure_kpa.is_some()
                || p.control_module_voltage.is_some()
                || p.engine_on_time.is_some()
                || p.lambda_cmd.is_some()
                || p.atmospheric_pressure.is_some()
                || p.intake_air_temperature.is_some()
                || p.mass_air_flow.is_some()
        });

        let line_smooth = if want_smooth {
            LINE_SMOOTH_VISUAL
        } else {
            0.0
        };
        Some((labels, times, sections, has_obd, line_smooth))
    });

    view! {
        <div class="telemetry-dashboard">
            {move || {
                match model.get() {
                    None => view! {
                        <div class="empty-state compact">
                            <Icon name="chart-line" size=IconSize::Lg color=IconColor::Accent />
                            <div>"No samples for this trip yet."</div>
                        </div>
                    }.into_any(),
                    Some((_, _, sections, _, _)) if sections.is_empty() => view! {
                        <div class="empty-state compact">
                            <Icon name="chart-line" size=IconSize::Lg color=IconColor::Accent />
                            <div>"No chartable telemetry in these samples."</div>
                        </div>
                    }.into_any(),
                    Some((labels, times, sections, has_obd, line_smooth)) => {
                        let labels = labels.clone();
                        let times = times.clone();
                        let smooth_tag = if line_smooth > 0.0 { "s" } else { "r" };
                        let chip_list = chips.get();
                        let show_chips = !chip_list.is_empty();
                        // Flatten panels with section key for category filtering.
                        let flat: Vec<(
                            String,
                            String,
                            String,
                            bool,
                            String,
                            Option<String>,
                            Vec<ChartSeriesSpec>,
                            PanelKind,
                            String,
                        )> = sections
                            .iter()
                            .flat_map(|(_title, _icon, key, _blurb, panels)| {
                                panels.iter().map(|p| {
                                    (
                                        p.id.to_string(),
                                        p.title.to_string(),
                                        p.blurb.to_string(),
                                        p.primary,
                                        p.y_left.to_string(),
                                        p.y_right.clone(),
                                        p.series.clone(),
                                        p.kind,
                                        (*key).to_string(),
                                    )
                                })
                            })
                            .collect();
                        let available_keys: std::collections::HashSet<String> = sections
                            .iter()
                            .map(|s| s.2.to_string())
                            .collect();
                        let cat_now = category.get();
                        let filtered: Vec<_> = flat
                            .into_iter()
                            .filter(|(_, _, _, primary, _, _, _, _, sec)| match cat_now.as_str() {
                                "overview" => *primary,
                                "all" => true,
                                other => sec == other,
                            })
                            .collect();
                        let filter_empty = filtered.is_empty();
                        let cat_note = match cat_now.as_str() {
                            "overview" => "Key trends for this trip",
                            "all" => "Every available metric",
                            "drive" => "Pace, pedal, and GPS quality",
                            "engine" => "RPM, load, and airflow",
                            "fuel" => "Burn rate, tank, and mixture",
                            "thermal" => "Temps, voltage, and pressure",
                            _ => "Trip charts",
                        };

                        view! {
                            <Show when=move || !has_obd>
                                <div class="info-banner">
                                    <Icon name="info" size=IconSize::Sm color=IconColor::Accent />
                                    <span>"GPS track only — no OBD telemetry was recorded for this trip."</span>
                                </div>
                            </Show>

                            {if show_chips {
                                view! {
                                    <div class="telemetry-signal-strip" role="group" aria-label="Trip signal summary">
                                        <For
                                            each=move || chip_list.clone()
                                            key=|c| c.label.clone()
                                            children=move |chip| {
                                                view! {
                                                    <div class="telemetry-signal-chip">
                                                        <span class="telemetry-signal-label">{chip.label}</span>
                                                        <span class="telemetry-signal-value">{chip.value}</span>
                                                    </div>
                                                }
                                            }
                                        />
                                    </div>
                                }.into_any()
                            } else {
                                view! { <></> }.into_any()
                            }}

                            <div class="telemetry-toolbar">
                                <div class="telemetry-cat-scroll" role="tablist" aria-label="Telemetry category">
                                    {CATEGORY_TABS
                                        .iter()
                                        .filter(|(key, _)| {
                                            *key == "overview"
                                                || *key == "all"
                                                || available_keys.contains(*key)
                                        })
                                        .map(|(key, label)| {
                                            let key_s = (*key).to_string();
                                            let key_active = key_s.clone();
                                            let key_click = key_s.clone();
                                            let label_s = (*label).to_string();
                                            view! {
                                                <button
                                                    type="button"
                                                    role="tab"
                                                    class=move || {
                                                        if category.get() == key_active {
                                                            "telemetry-cat-btn is-active"
                                                        } else {
                                                            "telemetry-cat-btn"
                                                        }
                                                    }
                                                    prop:aria-selected=move || category.get() == key_s
                                                    on:click=move |_| {
                                                        category.set(key_click.clone());
                                                        save_category_filter(&key_click);
                                                    }
                                                >
                                                    {label_s}
                                                </button>
                                            }
                                        })
                                        .collect_view()}
                                </div>
                                <div class="telemetry-toolbar-right">
                                    <div class="seg-control" role="group" aria-label="Chart smoothing">
                                        <button
                                            type="button"
                                            class=move || {
                                                if smooth.get() {
                                                    "seg-btn is-active"
                                                } else {
                                                    "seg-btn"
                                                }
                                            }
                                            prop:aria-pressed=move || smooth.get()
                                            on:click=move |_| smooth.set(true)
                                        >
                                            "Smooth"
                                        </button>
                                        <button
                                            type="button"
                                            class=move || {
                                                if !smooth.get() {
                                                    "seg-btn is-active"
                                                } else {
                                                    "seg-btn"
                                                }
                                            }
                                            prop:aria-pressed=move || !smooth.get()
                                            on:click=move |_| smooth.set(false)
                                        >
                                            "Raw"
                                        </button>
                                    </div>
                                </div>
                            </div>
                            <p class="muted telemetry-toolbar-note">
                                {format!("{cat_note} · zoom slider · click map or chart to pin time")}
                            </p>

                            {if filter_empty {
                                view! {
                                    <div class="empty-state compact">
                                        <div>"No charts in this category for this trip."</div>
                                    </div>
                                }.into_any()
                            } else {
                                view! {
                                    <div class="telemetry-panels">
                                        <For
                                            each=move || filtered.clone()
                                            key=move |p| format!("{}-{}-{smooth_tag}", p.0, category.get_untracked())
                                            children=move |(id, panel_title, blurb, primary, y_left, y_right, series, kind, _sec)| {
                                                let labels = labels.clone();
                                                let times = times.clone();
                                                let chart_id = format!("tel-{id}-{smooth_tag}");
                                                let panel_class = if primary {
                                                    "card telemetry-panel telemetry-panel-primary"
                                                } else {
                                                    "card telemetry-panel"
                                                };
                                                view! {
                                                    <div class=panel_class>
                                                        <div class="telemetry-panel-head">
                                                            <h3 class="telemetry-panel-title">{panel_title}</h3>
                                                            <details class="telemetry-help">
                                                                <summary title="About this metric" aria-label="About this metric">"ⓘ"</summary>
                                                                <p>{blurb}</p>
                                                            </details>
                                                        </div>
                                                        <TelemetryChart
                                                            chart_id=chart_id
                                                            labels=labels
                                                            times=times
                                                            series=series
                                                            y_left_name=y_left
                                                            y_right_name=y_right
                                                            kind=kind
                                                            line_smooth=line_smooth
                                                        />
                                                    </div>
                                                }
                                            }
                                        />
                                    </div>
                                }.into_any()
                            }}
                        }.into_any()
                    }
                }
            }}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ema_empty() {
        assert!(ema_series(&[], SMOOTH_SPAN).is_empty());
    }

    #[test]
    fn ema_preserves_first_and_len() {
        let data = vec![Some(10.0), Some(20.0), Some(30.0)];
        let out = ema_series(&data, SMOOTH_SPAN);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], Some(10.0));
        assert!(out[2].unwrap() < 30.0);
        assert!(out[2].unwrap() > 10.0);
    }

    #[test]
    fn ema_resets_after_null_gap() {
        let data = vec![Some(10.0), Some(20.0), None, Some(100.0), Some(100.0)];
        let out = ema_series(&data, SMOOTH_SPAN);
        assert_eq!(out[2], None);
        assert_eq!(out[3], Some(100.0));
    }

    #[test]
    fn maybe_smooth_is_stronger_than_single_ema() {
        let mut data = Vec::new();
        for i in 0..80 {
            // Square-ish noise: alternate high/low bursts
            let v = if (i / 3) % 2 == 0 { 10.0 } else { 40.0 };
            data.push(Some(v));
        }
        let single = ema_series(&data, SMOOTH_SPAN);
        let double = maybe_smooth_series(&data, true);
        let raw = maybe_smooth_series(&data, false);
        assert_eq!(raw, data);
        // Double pass should sit closer to the mid-band than a single pass late in series.
        let s = single[70].unwrap();
        let d = double[70].unwrap();
        let mid = 25.0;
        assert!((d - mid).abs() <= (s - mid).abs() + 1e-9);
    }

    #[test]
    fn y_axis_color_matches_bound_series_palette() {
        let colors = ["#3b82f6", "#22c55e", "#f59e0b"];
        let series = vec![
            ChartSeriesSpec {
                name: "speed".into(),
                data: vec![Some(1.0)],
                y_axis_index: 0,
                area: false,
            },
            ChartSeriesSpec {
                name: "pedal".into(),
                data: vec![Some(2.0)],
                y_axis_index: 1,
                area: false,
            },
        ];
        assert_eq!(y_axis_series_color(&series, &colors, 0), "#3b82f6");
        assert_eq!(y_axis_series_color(&series, &colors, 1), "#22c55e");
        // Single-axis: only right-tagged series still tints left from first series.
        let only_right = vec![ChartSeriesSpec {
            name: "x".into(),
            data: vec![Some(1.0)],
            y_axis_index: 1,
            area: false,
        }];
        assert_eq!(y_axis_series_color(&only_right, &colors, 0), "#3b82f6");
        assert_eq!(y_axis_series_color(&only_right, &colors, 1), "#3b82f6");
    }
}

