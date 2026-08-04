use leptos::prelude::*;
use wasm_bindgen::prelude::*;

use crate::api::TripPoint;

#[wasm_bindgen(inline_js = r#"
export function renderTripCharts(elId, series) {
  const el = document.getElementById(elId);
  if (!el || !window.echarts) return;
  const chart = echarts.init(el, 'dark');
  chart.setOption({
    backgroundColor: 'transparent',
    tooltip: { trigger: 'axis' },
    legend: { data: series.names },
    grid: { left: 40, right: 20, top: 40, bottom: 30 },
    xAxis: { type: 'category', data: series.labels },
    yAxis: { type: 'value' },
    series: series.names.map((name, idx) => ({
      name,
      type: 'line',
      showSymbol: false,
      data: series.values[idx]
    }))
  });
  window.addEventListener('resize', () => chart.resize());
}
"#)]
extern "C" {
    fn renderTripCharts(el_id: &str, series: &JsValue);
}

#[component]
pub fn TripCharts(points: Signal<Vec<TripPoint>>) -> impl IntoView {
    let id = "trip-charts";
    Effect::new(move |_| {
        let pts = points.get();
        if pts.is_empty() {
            return;
        }
        let labels: Vec<String> = pts
            .iter()
            .map(|p| p.recorded_at.clone())
            .collect();
        let speed: Vec<Option<f64>> = pts.iter().map(|p| p.vehicle_speed_kph).collect();
        let rpm: Vec<Option<f64>> = pts.iter().map(|p| p.vehicle_engine_rpm).collect();
        let fuel: Vec<Option<f64>> = pts.iter().map(|p| p.fuel_consumption_rate).collect();
        let load: Vec<Option<f64>> = pts.iter().map(|p| p.engine_load_pct).collect();
        let payload = serde_json::json!({
            "labels": labels,
            "names": ["Speed (km/h)", "RPM", "Fuel L/h", "Load %"],
            "values": [speed, rpm, fuel, load]
        });
        if let Ok(js) = js_sys::JSON::parse(&payload.to_string()) {
            renderTripCharts(id, &js);
        }
    });
    view! { <div id=id class="chart"></div> }
}
