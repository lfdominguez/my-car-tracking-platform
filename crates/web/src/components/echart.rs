//! A plain ECharts panel for pages beyond the trip cockpit (statistics, trip
//! comparison). ECharts is loaded on first use from the self-hosted vendor copy,
//! like the telemetry charts, and each instance is disposed with its component.
//! Unlike the telemetry charts these take part in no chart/map selection sync.

use leptos::prelude::*;
use wasm_bindgen::prelude::*;

use crate::components::charts::{CHART_FONT_NUM, chart_theme};

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

const __plainCharts = new Map();
const __plainPending = new Map();

export function renderPlainChart(elId, optionJson, appTheme) {
  if (!window.echarts) {
    __plainPending.set(elId, [optionJson, appTheme]);
    loadVendorScript('/vendor/echarts.min.js', 'echarts')
      .then(() => {
        const p = __plainPending.get(elId);
        __plainPending.delete(elId);
        if (p) renderPlainChart(elId, ...p);
      })
      .catch((err) => console.error('ECharts failed to load', err));
    return;
  }
  const el = document.getElementById(elId);
  if (!el) return;
  let option;
  try { option = JSON.parse(optionJson); } catch (_) { return; }
  let entry = __plainCharts.get(elId);
  if (entry && (entry.theme !== appTheme || entry.el !== el)) {
    disposePlainChart(elId);
    entry = null;
  }
  if (!entry) {
    const chart = echarts.init(el, appTheme === 'dark' ? 'dark' : null, { renderer: 'canvas' });
    entry = { chart, theme: appTheme, el };
    if (typeof ResizeObserver !== 'undefined') {
      entry.ro = new ResizeObserver(() => requestAnimationFrame(() => {
        try { chart.resize(); } catch (_) {}
      }));
      entry.ro.observe(el);
    }
    __plainCharts.set(elId, entry);
  }
  entry.chart.setOption(option, { notMerge: true });
}

export function disposePlainChart(elId) {
  __plainPending.delete(elId);
  const entry = __plainCharts.get(elId);
  if (!entry) return;
  try { entry.ro && entry.ro.disconnect(); } catch (_) {}
  try { entry.chart.dispose(); } catch (_) {}
  __plainCharts.delete(elId);
}
"#)]
extern "C" {
    fn renderPlainChart(el_id: &str, option_json: &str, app_theme: &str);
    fn disposePlainChart(el_id: &str);
}

/// An ECharts panel. `option` is rebuilt (and re-applied) whenever it changes or
/// the app theme flips, so builders should read colors from [`chart_chrome`].
#[component]
pub fn EChart(
    id: &'static str,
    #[prop(into)] option: Signal<Option<serde_json::Value>>,
    #[prop(optional, into)] class: Option<String>,
    /// i18n key of the text alternative for screen readers.
    #[prop(optional)]
    label: Option<&'static str>,
) -> impl IntoView {
    let theme = crate::components::use_theme();
    on_cleanup(move || disposePlainChart(id));
    Effect::new(move |_| {
        let app_theme = match theme.theme.get() {
            crate::components::theme::Theme::Dark => "dark",
            crate::components::theme::Theme::Light => "light",
        };
        let Some(Some(opt)) = option.try_get() else {
            return;
        };
        if let Ok(json) = serde_json::to_string(&opt) {
            renderPlainChart(id, &json, app_theme);
        }
    });
    let class = format!("chart {}", class.unwrap_or_default());
    view! { <div id=id class=class role="img" aria-label=move || label.map(crate::i18n::t)></div> }
}

/// Shared chrome for [`EChart`] options, in the app palette.
pub struct ChartChrome {
    pub series: Vec<String>,
    pub muted: String,
    pub tooltip: serde_json::Value,
    pub grid: serde_json::Value,
    pub axis_label: serde_json::Value,
    pub split_line: serde_json::Value,
    pub axis_line: serde_json::Value,
}

pub fn chart_chrome() -> ChartChrome {
    let th = chart_theme();
    ChartChrome {
        tooltip: serde_json::json!({
            "trigger": "axis",
            "backgroundColor": th.tooltip_bg,
            "borderColor": th.border,
            "borderWidth": 1,
            "textStyle": { "color": th.ink, "fontSize": 12, "fontFamily": CHART_FONT_NUM },
            "extraCssText": "border-radius:12px;box-shadow:0 12px 32px -12px rgba(0,0,0,0.55);",
        }),
        grid: serde_json::json!({ "left": 56, "right": 20, "top": 36, "bottom": 36, "containLabel": false }),
        axis_label: serde_json::json!({ "color": th.muted, "fontFamily": CHART_FONT_NUM, "hideOverlap": true }),
        split_line: serde_json::json!({ "lineStyle": { "color": th.grid_line } }),
        axis_line: serde_json::json!({ "lineStyle": { "color": th.border } }),
        series: th.series,
        muted: th.muted,
    }
}
