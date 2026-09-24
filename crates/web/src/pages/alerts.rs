//! Alert rules of one car (#110): one row per kind with an on/off switch and a
//! threshold. Rules are personal (each viewer of the car sets their own); the
//! server stores thresholds in SI, converted here for imperial speed and
//! temperature.

use leptos::prelude::*;

use crate::api::{
    AlertRule, delete_alert_rule, list_alert_rules, toggle_alert_rule, upsert_alert_rule,
};
use crate::components::{Icon, IconColor, IconSize};
use crate::units::{UnitSystem, use_unit_prefs};

/// `(kind, title, description, default SI threshold, icon)`.
const KINDS: [(&str, &str, &str, f64, &str); 6] = [
    (
        "speeding",
        "Speeding",
        "Driving faster than",
        120.0,
        "speedometer",
    ),
    (
        "low_voltage",
        "Low battery voltage",
        "12 V system below",
        11.8,
        "battery-warning",
    ),
    (
        "coolant_high",
        "Engine running hot",
        "Coolant above",
        110.0,
        "thermometer-hot",
    ),
    ("low_fuel", "Low fuel", "Tank level below", 15.0, "gas-pump"),
    (
        "device_offline",
        "Tracker offline",
        "No data for more than",
        3.0,
        "wifi-slash",
    ),
    (
        "trip_open",
        "Trip left open",
        "A trip recording for more than",
        6.0,
        "clock-countdown",
    ),
];

/// SI threshold → what the user types, and its unit label.
fn to_display(kind: &str, v: f64, system: UnitSystem) -> (f64, &'static str) {
    match (kind, system) {
        ("speeding", UnitSystem::Us) => (v / crate::units::KM_PER_MILE, "mph"),
        ("speeding", UnitSystem::Metric) => (v, "km/h"),
        ("coolant_high", UnitSystem::Us) => (v * 9.0 / 5.0 + 32.0, "°F"),
        ("coolant_high", UnitSystem::Metric) => (v, "°C"),
        ("low_voltage", _) => (v, "V"),
        ("low_fuel", _) => (v, "%"),
        ("device_offline", _) => (v, "days"),
        _ => (v, "hours"),
    }
}

/// What the user typed → SI.
fn to_si(kind: &str, v: f64, system: UnitSystem) -> f64 {
    match (kind, system) {
        ("speeding", UnitSystem::Us) => v * crate::units::KM_PER_MILE,
        ("coolant_high", UnitSystem::Us) => (v - 32.0) * 5.0 / 9.0,
        _ => v,
    }
}

fn fmt_threshold(v: f64) -> String {
    if (v - v.round()).abs() < 1e-6 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    }
}

#[component]
pub fn AlertsSection(#[prop(into)] car_id: Signal<String>) -> impl IntoView {
    let rules = RwSignal::new(Vec::<AlertRule>::new());
    let error = RwSignal::new(Option::<String>::None);

    Effect::new(move |_| {
        let id = car_id.get();
        if id.is_empty() {
            return;
        }
        leptos::task::spawn_local(async move {
            match list_alert_rules(&id).await {
                Ok(r) => {
                    let _ = rules.try_set(r);
                }
                Err(e) => {
                    let _ = error.try_set(Some(e.to_string()));
                }
            }
        });
    });

    view! {
        <section class="card alerts-card">
            <div class="telemetry-section-head">
                <h2 class="section-title">
                    <Icon name="bell-ringing" color=IconColor::Warn />
                    "Alerts"
                </h2>
                <span class="muted">"Personal to you · delivered to the bell and push"</span>
            </div>
            <Show when=move || error.get().is_some()>
                <div class="error">{move || error.get().unwrap_or_default()}</div>
            </Show>
            <ul class="alert-rules">
                {KINDS
                    .into_iter()
                    .map(|(kind, title, desc, default, icon)| view! {
                        <AlertRuleRow
                            car_id=car_id
                            kind=kind
                            title=title
                            desc=desc
                            default_si=default
                            icon=icon
                            rules=rules
                            error=error
                        />
                    })
                    .collect_view()}
            </ul>
        </section>
    }
}

#[component]
fn AlertRuleRow(
    car_id: Signal<String>,
    kind: &'static str,
    title: &'static str,
    desc: &'static str,
    default_si: f64,
    icon: &'static str,
    rules: RwSignal<Vec<AlertRule>>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let prefs = use_unit_prefs();
    let rule = Memo::new(move |_| rules.with(|r| r.iter().find(|x| x.kind == kind).cloned()));
    let input = RwSignal::new(String::new());
    let busy = RwSignal::new(false);

    // Show the stored (or default) threshold in the user's units.
    Effect::new(move |_| {
        let system = prefs.with(|p| p.system);
        let si = rule
            .with(|r| r.as_ref().map(|r| r.threshold))
            .unwrap_or(default_si);
        input.set(fmt_threshold(to_display(kind, si, system).0));
    });

    let enabled = move || rule.with(|r| r.as_ref().is_some_and(|r| r.enabled));
    let unit = move || to_display(kind, 0.0, prefs.with(|p| p.system)).1;

    let save = move |enable: bool| {
        let system = prefs.with_untracked(|p| p.system);
        let Some(v) = input
            .get_untracked()
            .trim()
            .replace(',', ".")
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite() && *v > 0.0)
        else {
            error.set(Some(format!("{title}: enter a threshold above zero.")));
            return;
        };
        let id = car_id.get_untracked();
        busy.set(true);
        leptos::task::spawn_local(async move {
            match upsert_alert_rule(&id, kind, to_si(kind, v, system), enable).await {
                Ok(saved) => {
                    let _ = rules.try_update(|r| {
                        r.retain(|x| x.kind != kind);
                        r.push(saved);
                    });
                    let _ = error.try_set(None);
                }
                Err(e) => {
                    let _ = error.try_set(Some(e.to_string()));
                }
            }
            let _ = busy.try_set(false);
        });
    };

    let toggle = move |on: bool| {
        let Some(r) = rule.get_untracked() else {
            save(on);
            return;
        };
        let id = car_id.get_untracked();
        busy.set(true);
        leptos::task::spawn_local(async move {
            match toggle_alert_rule(&id, &r.id, on).await {
                Ok(()) => {
                    let _ = rules.try_update(|list| {
                        if let Some(x) = list.iter_mut().find(|x| x.id == r.id) {
                            x.enabled = on;
                        }
                    });
                }
                Err(e) => {
                    let _ = error.try_set(Some(e.to_string()));
                }
            }
            let _ = busy.try_set(false);
        });
    };

    let remove = move |_| {
        let Some(r) = rule.get_untracked() else {
            return;
        };
        let id = car_id.get_untracked();
        busy.set(true);
        leptos::task::spawn_local(async move {
            match delete_alert_rule(&id, &r.id).await {
                Ok(()) => {
                    let _ = rules.try_update(|list| list.retain(|x| x.id != r.id));
                }
                Err(e) => {
                    let _ = error.try_set(Some(e.to_string()));
                }
            }
            let _ = busy.try_set(false);
        });
    };

    let input_id = format!("alert-{kind}");
    view! {
        <li class="alert-rule" class:is-on=enabled>
            <label class="alert-rule-toggle">
                <input
                    type="checkbox"
                    prop:checked=enabled
                    prop:disabled=move || busy.get()
                    on:change=move |ev| toggle(event_target_checked(&ev))
                />
                <span class="alert-rule-title">
                    <Icon name=icon size=IconSize::Sm color=IconColor::Accent />
                    {title}
                </span>
            </label>
            <div class="alert-rule-threshold">
                <label class="muted" for=input_id.clone()>{desc}</label>
                <input
                    id=input_id
                    type="number"
                    inputmode="decimal"
                    min="0"
                    step="any"
                    prop:value=move || input.get()
                    on:input=move |ev| input.set(event_target_value(&ev))
                />
                <span class="muted">{unit}</span>
            </div>
            <div class="alert-rule-actions">
                <button
                    type="button"
                    class="btn secondary btn-sm"
                    prop:disabled=move || busy.get()
                    on:click=move |_| save(true)
                >
                    {move || if rule.with(|r| r.is_some()) { "Save" } else { "Turn on" }}
                </button>
                <Show when=move || rule.with(|r| r.is_some())>
                    <button
                        type="button"
                        class="btn ghost btn-sm"
                        aria-label=format!("Remove the {title} alert")
                        prop:disabled=move || busy.get()
                        on:click=remove
                    >
                        <Icon name="trash" size=IconSize::Sm />
                    </button>
                </Show>
            </div>
        </li>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imperial_speed_and_temperature_round_trip() {
        let (mph, unit) = to_display("speeding", 120.0, UnitSystem::Us);
        assert_eq!(unit, "mph");
        assert!((to_si("speeding", mph, UnitSystem::Us) - 120.0).abs() < 1e-9);
        let (f, unit) = to_display("coolant_high", 100.0, UnitSystem::Us);
        assert_eq!((f, unit), (212.0, "°F"));
        assert!((to_si("coolant_high", 212.0, UnitSystem::Us) - 100.0).abs() < 1e-9);
        assert_eq!(to_display("low_voltage", 11.8, UnitSystem::Us), (11.8, "V"));
    }
}
