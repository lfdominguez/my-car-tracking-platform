//! Raw telemetry retention of one car (owner only): keep every sample, or prune
//! the per-second points of trips older than N days (N ≥ 30). Trip summaries,
//! statistics and a simplified route survive pruning; point graphs, replay and
//! export do not.

use leptos::prelude::*;

use crate::api::{Car, set_car_retention};
use crate::components::{Icon, IconColor, IconSize};

/// Smallest retention the server accepts.
const MIN_DAYS: i32 = 30;
const PRESETS: [(Option<i32>, &str); 4] = [
    (None, "Keep all"),
    (Some(90), "90 days"),
    (Some(180), "180 days"),
    (Some(365), "1 year"),
];

/// Parse the custom field: a whole number of days, at least [`MIN_DAYS`].
fn parse_days(raw: &str) -> Option<i32> {
    raw.trim().parse::<i32>().ok().filter(|d| *d >= MIN_DAYS)
}

#[component]
pub fn RetentionCard(car: RwSignal<Option<Car>>, error: RwSignal<Option<String>>) -> impl IntoView {
    let current = move || car.with(|c| c.as_ref().and_then(|c| c.raw_retention_days));
    // Pending choice; `None` inside = keep all.
    let choice = RwSignal::new(Option::<Option<i32>>::None);
    let custom = RwSignal::new(String::new());
    let custom_mode = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let saved = RwSignal::new(false);

    let selected = move || choice.get().unwrap_or_else(current);
    let is_preset = move |v: Option<i32>| !custom_mode.get() && selected() == v;

    let save = move |value: Option<i32>| {
        let Some(id) = car.with_untracked(|c| c.as_ref().map(|c| c.id.clone())) else {
            return;
        };
        if value.is_some()
            && !web_sys::window()
                .and_then(|w| {
                    w.confirm_with_message(
                        "Older trips will lose their per-second samples: point graphs, replay and export stop working for them. Continue?",
                    )
                    .ok()
                })
                .unwrap_or(false)
        {
            return;
        }
        busy.set(true);
        saved.set(false);
        leptos::task::spawn_local(async move {
            match set_car_retention(&id, value).await {
                Ok(v) => {
                    let _ = car.try_update(|c| {
                        if let Some(c) = c.as_mut() {
                            c.raw_retention_days = v;
                        }
                    });
                    let _ = choice.try_set(None);
                    let _ = saved.try_set(true);
                }
                Err(e) => {
                    let _ = error.try_set(Some(e.to_string()));
                }
            }
            let _ = busy.try_set(false);
        });
    };

    view! {
        <Show when=move || car.with(|c| c.as_ref().is_some_and(|c| c.role == "owner"))>
            <section class="card retention-card">
                <h2 class="section-title">
                    <Icon name="archive" color=IconColor::Accent />
                    "Raw data retention"
                </h2>
                <p class="muted">
                    {move || match current() {
                        None => "Every per-second sample is kept.".to_string(),
                        Some(d) => format!("Per-second samples are pruned from trips older than {d} days."),
                    }}
                </p>
                <div class="row">
                    <div class="seg-control" role="group" aria-label="Keep raw samples for">
                        {PRESETS
                            .into_iter()
                            .map(|(value, label)| view! {
                                <button type="button"
                                    class=move || if is_preset(value) { "seg-btn is-active" } else { "seg-btn" }
                                    aria-pressed=move || is_preset(value).to_string()
                                    on:click=move |_| {
                                        custom_mode.set(false);
                                        choice.set(Some(value));
                                    }>
                                    {label}
                                </button>
                            })
                            .collect_view()}
                        <button type="button"
                            class=move || {
                                let custom_selected = custom_mode.get()
                                    || selected().is_some_and(|d| ![90, 180, 365].contains(&d));
                                if custom_selected { "seg-btn is-active" } else { "seg-btn" }
                            }
                            on:click=move |_| {
                                custom_mode.set(true);
                                if custom.get_untracked().is_empty() {
                                    custom.set(selected().unwrap_or(730).to_string());
                                }
                            }>
                            "Custom"
                        </button>
                    </div>
                    <Show when=move || custom_mode.get()>
                        <label class="retention-custom">
                            <input type="number" min=MIN_DAYS step="1"
                                aria-label="Days to keep"
                                prop:value=move || custom.get()
                                on:input=move |ev| {
                                    custom.set(event_target_value(&ev));
                                    choice.set(Some(parse_days(&custom.get_untracked())));
                                } />
                            <span class="muted">"days"</span>
                        </label>
                    </Show>
                    <button type="button" class="btn primary btn-sm"
                        prop:disabled=move || {
                            busy.get()
                                || (custom_mode.get() && parse_days(&custom.get()).is_none())
                                || selected() == current() && !custom_mode.get()
                        }
                        on:click=move |_| {
                            let value = if custom_mode.get_untracked() {
                                match parse_days(&custom.get_untracked()) {
                                    Some(d) => Some(d),
                                    None => return,
                                }
                            } else {
                                selected()
                            };
                            save(value);
                        }>
                        <Icon name="floppy-disk" size=IconSize::Sm />
                        "Save"
                    </button>
                    <Show when=move || saved.get()>
                        <span class="muted" role="status">"Saved."</span>
                    </Show>
                </div>
                <p class="field-hint">
                    {format!("At least {MIN_DAYS} days. ")}
                    "Pruned trips keep their summary, statistics and a simplified route on the map, "
                    "but point graphs, replay and export stop working for them."
                </p>
            </section>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_days_respect_the_minimum() {
        assert_eq!(parse_days("30"), Some(30));
        assert_eq!(parse_days(" 400 "), Some(400));
        assert_eq!(parse_days("29"), None);
        assert_eq!(parse_days("abc"), None);
    }
}
