use leptos::prelude::*;
use wasm_bindgen::prelude::*;

use crate::api::TripPoint;
use crate::units::{instant_economy, use_unit_prefs, UnitSystem};
use crate::components::{Icon, IconColor, IconSize};

#[wasm_bindgen(inline_js = r#"
const __tripCharts = new Map();
const __tripChartTimes = new Map(); // elId -> full ISO timestamps aligned with category axis
const TRIP_CHART_GROUP = 'trip-telemetry';
let __tripSelection = null; // { iso, dataIndexHint }

function reconnectTripCharts() {
  if (!window.echarts || __tripCharts.size === 0) return;
  // Re-bind so tooltip / dataZoom stay linked as panels mount/unmount.
  echarts.connect(TRIP_CHART_GROUP);
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
    const exact = times.indexOf(iso);
    return exact;
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

function selectionMarkLine(dataIndex, label) {
  return {
    symbol: 'none',
    animation: false,
    label: {
      show: true,
      formatter: label || 'Selected',
      color: '#fdf2f8',
      backgroundColor: 'rgba(190, 24, 93, 0.92)',
      padding: [3, 6],
      borderRadius: 4,
      fontSize: 11
    },
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
  }, { lazyUpdate: true });
}

function applySelectionToChart(elId, chart, iso) {
  const times = __tripChartTimes.get(elId) || [];
  let dataIndex = nearestTimeIndex(times, iso);
  if (dataIndex < 0) {
    // Fall back to category length from option
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

  chart.setOption({
    series: series.map((s, i) => ({
      markLine: i === 0
        ? selectionMarkLine(dataIndex, label)
        : { symbol: 'none', label: { show: false }, lineStyle: { color: '#f472b6', width: 2 }, data: [{ xAxis: dataIndex }] }
    }))
  }, { lazyUpdate: true });

  try {
    chart.dispatchAction({
      type: 'showTip',
      seriesIndex: 0,
      dataIndex
    });
  } catch (_) {}

  return dataIndex;
}

function reapplyTripSelection() {
  if (!__tripSelection || !__tripSelection.iso) return;
  for (const [elId, chart] of __tripCharts.entries()) {
    applySelectionToChart(elId, chart, __tripSelection.iso);
  }
}

function selectTripTelemetryByTime(iso, dataIndexHint) {
  if (!iso) return;
  __tripSelection = { iso, dataIndexHint: dataIndexHint != null ? dataIndexHint : null };
  let resolved = null;
  for (const [elId, chart] of __tripCharts.entries()) {
    const idx = applySelectionToChart(elId, chart, iso);
    if (resolved == null && idx >= 0) resolved = idx;
  }
  if (resolved != null) {
    __tripSelection.dataIndexHint = resolved;
  }
  try {
    window.dispatchEvent(new CustomEvent('trip-telemetry-select', {
      detail: { iso, dataIndex: __tripSelection.dataIndexHint }
    }));
  } catch (_) {}
}

function clearTripTelemetrySelection() {
  __tripSelection = null;
  for (const chart of __tripCharts.values()) {
    clearMarkLines(chart);
    try {
      chart.dispatchAction({ type: 'hideTip' });
    } catch (_) {}
  }
  try {
    window.dispatchEvent(new CustomEvent('trip-telemetry-clear'));
  } catch (_) {}
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
    chart = echarts.init(el, 'dark');
    chart.group = TRIP_CHART_GROUP;
    __tripCharts.set(elId, chart);
    const onResize = () => {
      const c = __tripCharts.get(elId);
      if (c) c.resize();
    };
    window.addEventListener('resize', onResize);
    chart.__onResize = onResize;
    reconnectTripCharts();
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
  chart.setOption(option, { notMerge: true });
  requestAnimationFrame(() => {
    chart.resize();
    reconnectTripCharts();
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
  reconnectTripCharts();
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

#[derive(Clone, PartialEq)]
struct PanelDef {
    id: &'static str,
    title: &'static str,
    y_left: String,
    y_right: Option<String>,
    series: Vec<ChartSeriesSpec>,
}

fn series_has_data(data: &[Option<f64>]) -> bool {
    data.iter().any(|v| v.is_some())
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


fn build_option(
    title: &str,
    labels: &[String],
    times: &[String],
    series: &[ChartSeriesSpec],
    y_left_name: &str,
    y_right_name: Option<&str>,
) -> serde_json::Value {
    let colors = [
        "#3b82f6", "#22c55e", "#f59e0b", "#a78bfa", "#22d3ee", "#f472b6", "#eab308",
    ];
    let use_right = y_right_name.is_some() && series.iter().any(|s| s.y_axis_index == 1);

    // Axis unit names sit mid-axis so they never collide with clickable legend items.
    let mut y_axis = vec![serde_json::json!({
        "type": "value",
        "name": y_left_name,
        "nameLocation": "middle",
        "nameGap": 46,
        "nameRotate": 90,
        "nameTextStyle": { "color": "#93a0b8", "fontSize": 11, "padding": [0, 0, 0, 0] },
        "splitLine": { "lineStyle": { "color": "rgba(36,49,74,0.85)" } },
        "axisLabel": { "color": "#93a0b8", "hideOverlap": true },
        "axisLine": { "lineStyle": { "color": "#24314a" } },
        "scale": true
    })];
    if use_right {
        y_axis.push(serde_json::json!({
            "type": "value",
            "name": y_right_name.unwrap_or(""),
            "nameLocation": "middle",
            "nameGap": 46,
            "nameRotate": 90,
            "nameTextStyle": { "color": "#93a0b8", "fontSize": 11 },
            "splitLine": { "show": false },
            "axisLabel": { "color": "#93a0b8", "hideOverlap": true },
            "axisLine": { "lineStyle": { "color": "#24314a" } },
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
                "smooth": 0.25,
                "showSymbol": false,
                "connectNulls": false,
                "yAxisIndex": if use_right { s.y_axis_index } else { 0 },
                "data": s.data,
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
        "color": colors,
        "title": {
            "text": title,
            "left": 8,
            "top": 4,
            "textStyle": { "color": "#e8eefc", "fontSize": 13, "fontWeight": 600 }
        },
        "tooltip": {
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
        },
        // Series toggles sit in the top band; axis unit names are mid-axis (nameGap),
        // so clickable legend labels no longer collide with kph / rpm / °C / etc.
        "legend": {
            "top": 28,
            "right": 8,
            "left": 160,
            "orient": "horizontal",
            "align": "right",
            "itemGap": 14,
            "itemWidth": 14,
            "itemHeight": 8,
            "padding": [2, 4, 2, 8],
            "textStyle": { "color": "#93a0b8", "fontSize": 11 },
            "type": "scroll",
            "pageIconColor": "#93a0b8",
            "pageTextStyle": { "color": "#93a0b8" }
        },
        "grid": {
            "left": 62,
            "right": if use_right { 62 } else { 28 },
            "top": 72,
            "bottom": 58,
            "containLabel": false
        },
        "dataZoom": [
            {
                "type": "inside",
                "xAxisIndex": 0,
                "start": 0,
                "end": 100,
                "zoomOnMouseWheel": true,
                "moveOnMouseMove": true
            },
            {
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
            }
        ],
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

#[component]
fn TelemetryChart(
    chart_id: String,
    title: String,
    labels: Vec<String>,
    times: Vec<String>,
    series: Vec<ChartSeriesSpec>,
    y_left_name: String,
    y_right_name: Option<String>,
) -> impl IntoView {
    let el_id = chart_id.clone();
    let el_id_dispose = chart_id.clone();
    let title_c = title.clone();
    let labels_c = labels.clone();
    let times_c = times.clone();
    let series_c = series.clone();
    let y_left_c = y_left_name.clone();
    let y_right_c = y_right_name.clone();

    Effect::new(move |_| {
        if series_c.is_empty() || labels_c.is_empty() {
            return;
        }
        let option = build_option(
            &title_c,
            &labels_c,
            &times_c,
            &series_c,
            &y_left_c,
            y_right_c.as_deref(),
        );
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

/// Sectioned trip telemetry charts built from full OBD point payloads.
#[component]
pub fn TripTelemetryDashboard(points: Signal<Vec<TripPoint>>) -> impl IntoView {
    let prefs = use_unit_prefs();
    let model = Memo::new(move |_| {
        let unit_prefs = prefs.get();
        let system = unit_prefs.system;
        let ul = &unit_prefs.labels;
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

        let mut sections: Vec<(&'static str, &'static str, &'static str, Vec<PanelDef>)> =
            Vec::new();

        // Drive dynamics
        let drive = filter_panels(vec![
            PanelDef {
                id: "drive-speed",
                title: "Speed & pedal",
                y_left: ul.speed.to_string(),
                y_right: Some("%".to_string()),
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
                y_left: "m".to_string(),
                y_right: None,
                series: vec![ChartSeriesSpec {
                    name: "GPS accuracy (m)".to_string(),
                    data: gps_acc,
                    y_axis_index: 0,
                    area: true,
                }],
            },
        ]);
        if !drive.is_empty() {
            sections.push(("Drive dynamics", "path", "drive", drive));
        }

        // Engine
        let engine = filter_panels(vec![
            PanelDef {
                id: "engine-rpm-load",
                title: "RPM & load",
                y_left: "RPM".to_string(),
                y_right: Some("%".to_string()),
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
                y_left: "g/s".to_string(),
                y_right: Some("kPa".to_string()),
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
        if !engine.is_empty() {
            sections.push(("Engine", "cpu", "engine", engine));
        }

        // Fuel & mixture
        let fuel = filter_panels(vec![
            PanelDef {
                id: "fuel-rate",
                title: "Fuel rate & economy",
                y_left: ul.fuel_rate.to_string(),
                y_right: Some(ul.fuel_economy.to_string()),
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
                id: "fuel-level",
                title: "Fuel level",
                y_left: "%".to_string(),
                y_right: None,
                series: vec![ChartSeriesSpec {
                    name: "Fuel level (%)".to_string(),
                    data: fuel_level,
                    y_axis_index: 0,
                    area: true,
                }],
            },
            PanelDef {
                id: "fuel-mixture",
                title: "Mixture & trims",
                y_left: "%".to_string(),
                y_right: Some("λ".to_string()),
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
        ]);
        if !fuel.is_empty() {
            sections.push(("Fuel & mixture", "drop", "fuel", fuel));
        }

        // Thermal & electrical
        let thermal = filter_panels(vec![
            PanelDef {
                id: "thermal-temps",
                title: "Temperatures",
                y_left: "°C".to_string(),
                y_right: None,
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
                y_left: "V".to_string(),
                y_right: Some("kPa".to_string()),
                series: vec![
                    ChartSeriesSpec {
                        name: "Module voltage (V)".to_string(),
                        data: voltage,
                        y_axis_index: 0,
                        area: true,
                    },
                    // MAP is the pressure series phones already upload; baro is optional.
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
        if !thermal.is_empty() {
            sections.push(("Thermal & electrical", "lightning", "thermal", thermal));
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

        Some((labels, times, sections, has_obd))
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
                    Some((_, _, sections, _)) if sections.is_empty() => view! {
                        <div class="empty-state compact">
                            <Icon name="chart-line" size=IconSize::Lg color=IconColor::Accent />
                            <div>"No chartable telemetry in these samples."</div>
                        </div>
                    }.into_any(),
                    Some((labels, times, sections, has_obd)) => {
                        let labels = labels.clone();
                        let times = times.clone();
                        view! {
                            <Show when=move || !has_obd>
                                <div class="info-banner">
                                    <Icon name="info" size=IconSize::Sm color=IconColor::Accent />
                                    <span>"GPS track only — no OBD telemetry was recorded for this trip."</span>
                                </div>
                            </Show>
                            <For
                                each=move || {
                                    sections
                                        .iter()
                                        .enumerate()
                                        .map(|(i, (title, icon, key, panels))| {
                                            (
                                                i,
                                                (*title).to_string(),
                                                (*icon).to_string(),
                                                (*key).to_string(),
                                                panels
                                                    .iter()
                                                    .map(|p| {
                                                        (
                                                            p.id.to_string(),
                                                            p.title.to_string(),
                                                            p.y_left.to_string(),
                                                            p.y_right.clone(),
                                                            p.series.clone(),
                                                        )
                                                    })
                                                    .collect::<Vec<_>>(),
                                            )
                                        })
                                        .collect::<Vec<_>>()
                                }
                                key=|s| s.3.clone()
                                children=move |(idx, title, icon, key, panels)| {
                                    let labels = labels.clone();
                                    let times = times.clone();
                                    view! {
                                        <section class="telemetry-section" data-section=key>
                                            <div class="telemetry-section-head">
                                                <h2 class="section-title">
                                                    <Icon name=icon color=IconColor::Accent />
                                                    {title}
                                                </h2>
                                                <span class="muted section-index">{format!("{:02}", idx + 1)}</span>
                                            </div>
                                            <div class="telemetry-panels">
                                                <For
                                                    each=move || panels.clone()
                                                    key=|p| p.0.clone()
                                                    children=move |(id, panel_title, y_left, y_right, series)| {
                                                        let labels = labels.clone();
                                                        let times = times.clone();
                                                        let chart_id = format!("tel-{id}");
                                                        view! {
                                                            <div class="card telemetry-panel">
                                                                <TelemetryChart
                                                                    chart_id=chart_id
                                                                    title=panel_title
                                                                    labels=labels
                                                                    times=times
                                                                    series=series
                                                                    y_left_name=y_left
                                                                    y_right_name=y_right
                                                                />
                                                            </div>
                                                        }
                                                    }
                                                />
                                            </div>
                                        </section>
                                    }
                                }
                            />
                        }.into_any()
                    }
                }
            }}
        </div>
    }
}

