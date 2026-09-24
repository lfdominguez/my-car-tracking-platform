//! "Live" dashboard card (#108): where each car is now, updated over SSE.
//!
//! The list comes from `GET /api/cars/live`; `GET /api/cars/live/stream` then
//! pushes `position` events (same shape) and `stale` hints, on which the list is
//! fetched again. The `EventSource` and its listeners are owned by the card and
//! released when it unmounts, the same way the chat page handles its stream.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use leptos::prelude::*;
use leptos_router::components::A;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::api::{LIVE_STREAM_URL, LivePosition, list_live_positions};
use crate::components::geo::{LiveMap, LiveMarker};
use crate::components::{Icon, IconColor, IconSize};
use crate::pages::chat::SendWrapper;
use crate::units::{km_to_display, use_unit_prefs};

/// An SSE event name and the listener registered for it.
type NamedListener = (&'static str, Closure<dyn FnMut(web_sys::MessageEvent)>);

/// An open live `EventSource` with the listeners attached to it; dropping it
/// closes the connection and detaches them.
struct LiveFeed {
    es: web_sys::EventSource,
    listeners: Vec<NamedListener>,
}

impl Drop for LiveFeed {
    fn drop(&mut self) {
        self.es.close();
        for (name, handler) in &self.listeners {
            let _ = self
                .es
                .remove_event_listener_with_callback(name, handler.as_ref().unchecked_ref());
        }
    }
}

/// "just now", "4 min ago", "3 h ago", "2 d ago".
pub(crate) fn ago(iso: &str, now: chrono::DateTime<chrono::Utc>) -> String {
    let Ok(t) = chrono::DateTime::parse_from_rfc3339(iso.trim()) else {
        return "—".into();
    };
    let secs = now
        .signed_duration_since(t.with_timezone(&chrono::Utc))
        .num_seconds()
        .max(0);
    match secs {
        0..=59 => crate::i18n::t("ago.just_now").into(),
        60..=3599 => crate::i18n::tf("ago.minutes", &[("n", &(secs / 60))]),
        3600..=86_399 => crate::i18n::tf("ago.hours", &[("n", &(secs / 3600))]),
        _ => crate::i18n::tf("ago.days", &[("n", &(secs / 86_400))]),
    }
}

/// Keep the newest position per car.
fn upsert(list: &mut Vec<LivePosition>, pos: LivePosition) {
    match list.iter_mut().find(|p| p.car_id == pos.car_id) {
        Some(cur) => {
            if pos.recorded_at >= cur.recorded_at || pos.track_id != cur.track_id {
                *cur = pos;
            }
        }
        None => list.push(pos),
    }
}

#[component]
pub fn LiveCard(
    /// `(car_id, name)` for labels.
    #[prop(into)]
    cars: Signal<Vec<(String, String)>>,
) -> impl IntoView {
    let prefs = use_unit_prefs();
    let positions = RwSignal::new(Vec::<LivePosition>::new());
    let loaded = RwSignal::new(false);
    let now = RwSignal::new(chrono::Utc::now());
    let feed: RwSignal<Option<SendWrapper<LiveFeed>>> = RwSignal::new(None);
    let alive = Arc::new(AtomicBool::new(true));

    let refetch = move || {
        leptos::task::spawn_local(async move {
            if let Ok(list) = list_live_positions().await {
                let _ = positions.try_set(list);
            }
            let _ = loaded.try_set(true);
        });
    };
    refetch();

    // SSE: open once for the card's lifetime.
    if let Ok(es) = web_sys::EventSource::new(LIVE_STREAM_URL) {
        let on_position =
            Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |ev: web_sys::MessageEvent| {
                let Some(data) = ev.data().as_string() else {
                    return;
                };
                if let Ok(pos) = serde_json::from_str::<LivePosition>(&data) {
                    let _ = positions.try_update(|list| upsert(list, pos));
                }
            });
        let on_stale =
            Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |_ev: web_sys::MessageEvent| {
                refetch();
            });
        let listeners = vec![("position", on_position), ("stale", on_stale)];
        for (name, handler) in &listeners {
            let _ = es.add_event_listener_with_callback(name, handler.as_ref().unchecked_ref());
        }
        feed.set(Some(SendWrapper::new(LiveFeed { es, listeners })));
    }

    // Re-render "X ago" labels.
    {
        let alive = Arc::clone(&alive);
        leptos::task::spawn_local(async move {
            loop {
                gloo_timers::future::TimeoutFuture::new(30_000).await;
                if !alive.load(Ordering::SeqCst) || now.try_set(chrono::Utc::now()).is_some() {
                    break;
                }
            }
        });
    }

    on_cleanup(move || {
        alive.store(false, Ordering::SeqCst);
        if let Some(f) = feed.try_update(Option::take).flatten() {
            drop(f);
        }
    });

    let name_of = move |car_id: &str| {
        cars.with(|c| {
            c.iter()
                .find(|(id, _)| id == car_id)
                .map(|(_, n)| n.clone())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| crate::i18n::t("common.car").into())
        })
    };

    let markers = Signal::derive(move || {
        let t = now.get();
        positions.with(|list| {
            list.iter()
                .map(|p| {
                    let label = name_of(&p.car_id);
                    LiveMarker {
                        id: p.car_id.clone(),
                        lat: p.lat,
                        lon: p.lon,
                        heading: p.heading_deg,
                        title: format!("{label} · {}", ago(&p.recorded_at, t)),
                        label,
                        driving: p.trip_open,
                    }
                })
                .collect::<Vec<_>>()
        })
    });

    view! {
        <Show when=move || loaded.get() && !positions.get().is_empty()>
            <section class="card live-card">
                <div class="telemetry-section-head">
                    <h2 class="section-title">
                        <Icon name="broadcast" color=IconColor::Accent />
                        {tr!("live.title")}
                    </h2>
                    <span class="muted">{tr!("live.lead")}</span>
                </div>
                <LiveMap markers=markers />
                <ul class="live-list">
                    <For
                        each=move || positions.get()
                        key=|p| format!("{}:{}:{}", p.car_id, p.recorded_at, p.trip_open)
                        children=move |p| {
                            let name = name_of(&p.car_id);
                            let recorded = p.recorded_at.clone();
                            let seen = move || crate::i18n::tf("live.last_seen", &[("ago", &ago(&recorded, now.get()))]);
                            let speed_kph = p.speed_kph;
                            let speed = move || {
                                let pr = prefs.get();
                                speed_kph
                                    .map(|v| format!("{} {}", crate::i18n::num(km_to_display(v, pr.system), 0), pr.labels.speed))
                                    .unwrap_or_default()
                            };
                            let (battery_pct, fuel_pct) = (p.battery_soc_pct, p.fuel_level_pct);
                            let level = battery_pct.or(fuel_pct).is_some().then_some(move || {
                                let pct = |v: f64| crate::i18n::num(v, 0);
                                battery_pct
                                    .map(|v| crate::i18n::tf("live.battery_pct", &[("pct", &pct(v))]))
                                    .or_else(|| fuel_pct.map(|v| crate::i18n::tf("live.fuel_pct", &[("pct", &pct(v))])))
                                    .unwrap_or_default()
                            });
                            let trip_href = format!("/app/trips/{}", p.track_id);
                            let driving = p.trip_open;
                            view! {
                                <li class="live-item">
                                    <div class="live-item-main">
                                        <span class="live-item-name">{name}</span>
                                        {if driving {
                                            view! { <span class="pill pill-live">{tr!("live.driving")}</span> }.into_any()
                                        } else {
                                            view! { <span class="pill">{tr!("live.parked")}</span> }.into_any()
                                        }}
                                    </div>
                                    <div class="live-item-meta muted">
                                        <span>{seen}</span>
                                        {driving.then(|| view! { <span>{speed}</span> })}
                                        {level.map(|l| view! { <span>{l}</span> })}
                                    </div>
                                    {driving.then(|| view! {
                                        <A href=trip_href.clone()>
                                            <span class="icon-label">
                                                {tr!("live.open_trip")}
                                                <Icon name="caret-right" size=IconSize::Sm />
                                            </span>
                                        </A>
                                    })}
                                </li>
                            }
                        }
                    />
                </ul>
            </section>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(car: &str, at: &str) -> LivePosition {
        LivePosition {
            car_id: car.into(),
            track_id: "t".into(),
            trip_open: true,
            recorded_at: at.into(),
            lat: 0.0,
            lon: 0.0,
            speed_kph: None,
            heading_deg: None,
            fuel_level_pct: None,
            battery_soc_pct: None,
        }
    }

    #[test]
    fn upsert_keeps_the_newest_fix_per_car() {
        let mut list = vec![pos("a", "2026-01-01T08:00:05Z")];
        upsert(&mut list, pos("a", "2026-01-01T08:00:01Z"));
        assert_eq!(list[0].recorded_at, "2026-01-01T08:00:05Z");
        upsert(&mut list, pos("a", "2026-01-01T08:00:09Z"));
        assert_eq!(list[0].recorded_at, "2026-01-01T08:00:09Z");
        upsert(&mut list, pos("b", "2026-01-01T08:00:00Z"));
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn ago_is_coarse() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-01-01T10:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert_eq!(ago("2026-01-01T09:59:30Z", now), "just now");
        assert_eq!(ago("2026-01-01T09:55:00Z", now), "5 min ago");
        assert_eq!(ago("2026-01-01T07:00:00Z", now), "3 h ago");
        crate::i18n::with_locale(crate::i18n::Locale::Es, || {
            assert_eq!(ago("2026-01-01T09:55:00Z", now), "hace 5 min");
        });
    }
}
