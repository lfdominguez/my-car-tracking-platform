//! Trip replay (#119): play a trip back at 1×, 10× or 60×.
//!
//! Rather than a marker of its own, replay drives the existing chart ↔ map
//! selection: each step calls `window.__tripTelemetry.selectByTime`, which moves
//! the chart cursor and announces `trip-telemetry-select`, on which the map moves
//! its pin to the nearest sample. Pausing leaves that sample pinned, so "Split
//! here" and the chart tooltips work from the replay position too.

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::api::TripPoint;
use crate::components::{Icon, IconSize};

/// Replay clock tick.
const TICK_MS: u32 = 100;
const SPEEDS: [u32; 3] = [1, 10, 60];

/// `(seconds from start, recorded_at)` of every sample with a timestamp.
fn timeline(points: &[TripPoint]) -> Vec<(f64, String)> {
    let parsed: Vec<(i64, &str)> = points
        .iter()
        .filter_map(|p| {
            chrono::DateTime::parse_from_rfc3339(p.recorded_at.trim())
                .ok()
                .map(|t| (t.timestamp_millis(), p.recorded_at.as_str()))
        })
        .collect();
    let Some(t0) = parsed.first().map(|(t, _)| *t) else {
        return Vec::new();
    };
    parsed
        .into_iter()
        .map(|(t, iso)| ((t - t0) as f64 / 1000.0, iso.to_string()))
        .collect()
}

/// Index of the last sample at or before `secs`.
fn index_at(tl: &[(f64, String)], secs: f64) -> usize {
    tl.partition_point(|(t, _)| *t <= secs).saturating_sub(1)
}

fn fmt_clock(secs: f64) -> String {
    let s = secs.max(0.0).round() as i64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Move the chart cursor (and, through its event, the map pin) to `iso`.
fn select_time(iso: &str) {
    let Some(win) = web_sys::window() else {
        return;
    };
    let Ok(bridge) = js_sys::Reflect::get(win.as_ref(), &"__tripTelemetry".into()) else {
        return;
    };
    let Some(f) = js_sys::Reflect::get(&bridge, &"selectByTime".into())
        .ok()
        .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
    else {
        return;
    };
    let _ = f.call3(
        &bridge,
        &iso.into(),
        &wasm_bindgen::JsValue::NULL,
        &wasm_bindgen::JsValue::FALSE,
    );
}

#[component]
pub fn TripReplay(#[prop(into)] points: Signal<Vec<TripPoint>>) -> impl IntoView {
    let tl = Memo::new(move |_| points.with(|p| timeline(p)));
    let total = Memo::new(move |_| tl.with(|t| t.last().map(|(s, _)| *s).unwrap_or(0.0)));
    let pos = RwSignal::new(0.0f64);
    let playing = RwSignal::new(false);
    let speed = RwSignal::new(10u32);
    // Last index pushed to the charts, so a tick that stays on the same sample
    // does not re-dispatch.
    let shown = RwSignal::new(usize::MAX);

    // A different trip starts over.
    Effect::new(move |_| {
        tl.track();
        playing.set(false);
        pos.set(0.0);
        shown.set(usize::MAX);
    });

    let show = move |secs: f64| {
        tl.with_untracked(|t| {
            if t.is_empty() {
                return;
            }
            let i = index_at(t, secs);
            if shown.get_untracked() != i {
                shown.set(i);
                select_time(&t[i].1);
            }
        });
    };

    let play = move || {
        if playing.get_untracked() {
            return;
        }
        if pos.get_untracked() >= total.get_untracked() {
            pos.set(0.0);
        }
        playing.set(true);
        leptos::task::spawn_local(async move {
            loop {
                gloo_timers::future::TimeoutFuture::new(TICK_MS).await;
                // Stops when paused, and when the page is gone (disposed → None).
                if playing.try_get_untracked() != Some(true) {
                    break;
                }
                let step = TICK_MS as f64 / 1000.0 * speed.get_untracked() as f64;
                let end = total.get_untracked();
                let next = (pos.get_untracked() + step).min(end);
                pos.set(next);
                show(next);
                if next >= end {
                    playing.set(false);
                    break;
                }
            }
        });
    };

    view! {
        <Show when=move || { tl.with(|t| t.len() >= 2) }>
            <div class="replay-bar" role="group" aria-label="Trip replay">
                <button
                    type="button"
                    class="btn secondary btn-sm replay-play"
                    aria-label=move || if playing.get() { "Pause replay" } else { "Play replay" }
                    on:click=move |_| {
                        if playing.get_untracked() {
                            playing.set(false);
                        } else {
                            play();
                        }
                    }
                >
                    {move || if playing.get() {
                        view! { <Icon name="pause" size=IconSize::Sm /> }.into_any()
                    } else {
                        view! { <Icon name="play" size=IconSize::Sm /> }.into_any()
                    }}
                    {move || if playing.get() { "Pause" } else { "Replay" }}
                </button>
                <input
                    type="range"
                    class="replay-seek"
                    min="0"
                    step="1"
                    aria-label="Replay position"
                    prop:max=move || total.get().ceil().to_string()
                    prop:value=move || pos.get().round().to_string()
                    on:input=move |ev| {
                        if let Ok(v) = event_target_value(&ev).parse::<f64>() {
                            pos.set(v);
                            show(v);
                        }
                    }
                />
                <span class="replay-clock">
                    {move || format!("{} / {}", fmt_clock(pos.get()), fmt_clock(total.get()))}
                </span>
                <div class="seg-control" role="group" aria-label="Replay speed">
                    {SPEEDS
                        .into_iter()
                        .map(|s| view! {
                            <button
                                type="button"
                                class=move || if speed.get() == s { "seg-btn is-active" } else { "seg-btn" }
                                aria-pressed=move || (speed.get() == s).to_string()
                                on:click=move |_| speed.set(s)
                            >
                                {format!("{s}×")}
                            </button>
                        })
                        .collect_view()}
                </div>
            </div>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_at_picks_the_sample_at_or_before() {
        let tl = vec![(0.0, "a".into()), (1.0, "b".into()), (5.0, "c".into())];
        assert_eq!(index_at(&tl, 0.0), 0);
        assert_eq!(index_at(&tl, 0.5), 0);
        assert_eq!(index_at(&tl, 1.0), 1);
        assert_eq!(index_at(&tl, 4.9), 1);
        assert_eq!(index_at(&tl, 99.0), 2);
    }

    #[test]
    fn clock_formats_minutes_and_hours() {
        assert_eq!(fmt_clock(65.0), "1:05");
        assert_eq!(fmt_clock(3725.0), "1:02:05");
    }
}
