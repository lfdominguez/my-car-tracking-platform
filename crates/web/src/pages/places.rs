//! Places (#111): circles and polygons that name trip ends ("Home → Office") and
//! can notify on enter / exit.
//!
//! Draw on the map: in circle mode a click sets the center and a slider the
//! radius; in polygon mode each click adds a vertex. Places belong to the
//! signed-in user and apply to one car or to all of them.

use leptos::prelude::*;
use leptos_router::components::A;

use crate::api::{
    Car, Geofence, GeofenceEvent, create_geofence, delete_geofence, geofence_events, list_cars,
    list_geofences, update_geofence,
};
use crate::components::geo::{AreasData, AreasMap, MapArea, circle_ring};
use crate::components::{Icon, IconColor, IconSize};

const PLACES_MAP_ID: &str = "places-map";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Circle,
    Polygon,
}

fn confirm(msg: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.confirm_with_message(msg).ok())
        .unwrap_or(false)
}

/// Outline of a stored place.
fn ring_of(g: &Geofence) -> Vec<[f64; 2]> {
    if let Some(p) = g.polygon.as_ref().filter(|p| p.len() >= 3) {
        return p.clone();
    }
    match (g.center_lat, g.center_lon, g.radius_m) {
        (Some(lat), Some(lon), Some(r)) => circle_ring(lat, lon, r),
        _ => Vec::new(),
    }
}

fn shape_label(g: &Geofence) -> String {
    match (&g.polygon, g.radius_m) {
        (Some(p), _) if p.len() >= 3 => format!("Area · {} points", p.len()),
        (_, Some(r)) if r >= 1000.0 => format!("Circle · {:.1} km", r / 1000.0),
        (_, Some(r)) => format!("Circle · {r:.0} m"),
        _ => "—".into(),
    }
}

#[component]
pub fn PlacesPage() -> impl IntoView {
    let places = RwSignal::new(Vec::<Geofence>::new());
    let cars = RwSignal::new(Vec::<Car>::new());
    let error = RwSignal::new(Option::<String>::None);
    let notice = RwSignal::new(Option::<String>::None);
    let busy = RwSignal::new(false);
    let refresh = RwSignal::new(0u32);

    // Draft shape.
    let shape = RwSignal::new(Shape::Circle);
    let center = RwSignal::new(Option::<[f64; 2]>::None);
    let radius = RwSignal::new(200.0f64);
    let vertices = RwSignal::new(Vec::<[f64; 2]>::new());
    let name = RwSignal::new(String::new());
    let car_id = RwSignal::new(String::new());
    let notify = RwSignal::new(false);
    let editing = RwSignal::new(Option::<String>::None);
    let open_events = RwSignal::new(Option::<String>::None);
    let events = RwSignal::new(Vec::<GeofenceEvent>::new());

    Effect::new(move |_| {
        refresh.track();
        leptos::task::spawn_local(async move {
            match list_geofences().await {
                Ok(p) => {
                    let _ = places.try_set(p);
                }
                Err(e) => {
                    let _ = error.try_set(Some(e.to_string()));
                }
            }
        });
    });
    leptos::task::spawn_local(async move {
        if let Ok(c) = list_cars().await {
            let _ = cars.try_set(c);
        }
    });

    // Map clicks draw the draft.
    Effect::new(move |_| {
        let handle = window_event_listener_untyped("geo-map-click", move |ev| {
            let Ok(detail) = js_sys::Reflect::get(&ev, &"detail".into()) else {
                return;
            };
            let get = |k: &str| js_sys::Reflect::get(&detail, &k.into()).ok();
            if get("el").and_then(|v| v.as_string()).as_deref() != Some(PLACES_MAP_ID) {
                return;
            }
            let (Some(lon), Some(lat)) = (
                get("lon").and_then(|v| v.as_f64()),
                get("lat").and_then(|v| v.as_f64()),
            ) else {
                return;
            };
            match shape.get_untracked() {
                Shape::Circle => {
                    let _ = center.try_set(Some([lon, lat]));
                }
                Shape::Polygon => {
                    let _ = vertices.try_update(|v| v.push([lon, lat]));
                }
            }
        });
        on_cleanup(move || handle.remove());
    });

    let clear_draft = move || {
        editing.set(None);
        center.set(None);
        vertices.set(Vec::new());
        name.set(String::new());
        notify.set(false);
        car_id.set(String::new());
    };

    let draft_ring = move || -> Vec<[f64; 2]> {
        match shape.get() {
            Shape::Circle => center
                .get()
                .map(|[lon, lat]| circle_ring(lat, lon, radius.get()))
                .unwrap_or_default(),
            Shape::Polygon => vertices.get(),
        }
    };
    let draft_ready = move || match shape.get() {
        Shape::Circle => center.get().is_some(),
        Shape::Polygon => vertices.with(|v| v.len() >= 3),
    };

    let map_data = Signal::derive(move || {
        let edit = editing.get();
        let mut areas: Vec<MapArea> = places.with(|p| {
            p.iter()
                .filter(|g| edit.as_deref() != Some(g.id.as_str()))
                .map(|g| MapArea {
                    id: g.id.clone(),
                    ring: ring_of(g),
                    draft: false,
                    label: g.name.clone(),
                })
                .collect()
        });
        let ring = draft_ring();
        if ring.len() >= 3 {
            areas.push(MapArea {
                id: "draft".into(),
                ring,
                draft: true,
                label: name.get(),
            });
        }
        AreasData {
            areas,
            vertices: if shape.get() == Shape::Polygon {
                vertices.get()
            } else {
                Vec::new()
            },
            fit_key: places.with(|p| p.len().to_string()),
        }
    });

    let save = move |_| {
        let n = name.get_untracked().trim().to_string();
        if n.is_empty() || !untrack(draft_ready) {
            return;
        }
        let mut body = serde_json::json!({ "name": n, "notify": notify.get_untracked() });
        match shape.get_untracked() {
            Shape::Circle => {
                let Some([lon, lat]) = center.get_untracked() else {
                    return;
                };
                body["center_lat"] = lat.into();
                body["center_lon"] = lon.into();
                body["radius_m"] = radius.get_untracked().into();
            }
            Shape::Polygon => {
                body["polygon"] = serde_json::json!(vertices.get_untracked());
            }
        }
        let edit_id = editing.get_untracked();
        if edit_id.is_none() {
            let c = car_id.get_untracked();
            body["car_id"] = if c.is_empty() {
                serde_json::Value::Null
            } else {
                c.into()
            };
        }
        busy.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            let res = match edit_id {
                Some(id) => update_geofence(&id, &body).await,
                None => create_geofence(&body).await,
            };
            match res {
                Ok(g) => {
                    let _ = notice.try_set(Some(format!("Saved “{}”.", g.name)));
                    clear_draft();
                    refresh.update(|n| *n = n.wrapping_add(1));
                }
                Err(e) => {
                    let _ = error.try_set(Some(e.to_string()));
                }
            }
            let _ = busy.try_set(false);
        });
    };

    let car_name = move |id: &Option<String>| match id {
        None => "All cars".to_string(),
        Some(id) => cars.with(|c| {
            c.iter()
                .find(|c| &c.id == id)
                .map(|c| c.name.clone())
                .unwrap_or_else(|| "A car".into())
        }),
    };

    view! {
        <div class="topbar">
            <div>
                <h1 class="section-title">
                    <Icon name="map-pin-area" color=IconColor::Accent />
                    "Places"
                </h1>
                <p class="muted">"Name the places you drive between — trips show “Home → Office” — and get notified on arrival or departure"</p>
            </div>
        </div>
        <Show when=move || error.get().is_some()>
            <div class="error">{move || error.get().unwrap_or_default()}</div>
        </Show>
        <Show when=move || notice.get().is_some()>
            <div class="success" role="status">{move || notice.get().unwrap_or_default()}</div>
        </Show>

        <div class="places-layout">
            <section class="card places-map-card">
                <div class="places-toolbar">
                    <div class="seg-control" role="group" aria-label="Shape">
                        <button type="button"
                            class=move || if shape.get() == Shape::Circle { "seg-btn is-active" } else { "seg-btn" }
                            aria-pressed=move || (shape.get() == Shape::Circle).to_string()
                            on:click=move |_| shape.set(Shape::Circle)>"Circle"</button>
                        <button type="button"
                            class=move || if shape.get() == Shape::Polygon { "seg-btn is-active" } else { "seg-btn" }
                            aria-pressed=move || (shape.get() == Shape::Polygon).to_string()
                            on:click=move |_| shape.set(Shape::Polygon)>"Polygon"</button>
                    </div>
                    <Show
                        when=move || shape.get() == Shape::Circle
                        fallback=move || view! {
                            <span class="muted">{move || format!("{} points · click the map to add", vertices.get().len())}</span>
                            <button type="button" class="btn ghost btn-sm"
                                prop:disabled=move || vertices.get().is_empty()
                                on:click=move |_| vertices.update(|v| { v.pop(); })>"Undo"</button>
                            <button type="button" class="btn ghost btn-sm"
                                prop:disabled=move || vertices.get().is_empty()
                                on:click=move |_| vertices.set(Vec::new())>"Clear"</button>
                        }
                    >
                        <label class="places-radius">
                            <span class="muted">{move || if center.get().is_some() { "Radius" } else { "Click the map to place the center · radius" }}</span>
                            <input type="range" min="50" max="5000" step="25"
                                prop:value=move || radius.get().to_string()
                                on:input=move |ev| {
                                    if let Ok(v) = event_target_value(&ev).parse::<f64>() {
                                        radius.set(v);
                                    }
                                } />
                            <span class="places-radius-value">{move || format!("{:.0} m", radius.get())}</span>
                        </label>
                    </Show>
                </div>
                <AreasMap id=PLACES_MAP_ID data=map_data />
                <div class="places-form">
                    <label class="garage-field">
                        <span>"Name"</span>
                        <input type="text" maxlength="80" placeholder="Home"
                            prop:value=move || name.get()
                            on:input=move |ev| name.set(event_target_value(&ev)) />
                    </label>
                    <label class="garage-field">
                        <span>"Car"</span>
                        <select prop:value=move || { cars.track(); car_id.get() }
                            prop:disabled=move || editing.get().is_some()
                            on:change=move |ev| car_id.set(event_target_value(&ev))>
                            <option value="">"All cars"</option>
                            <For each=move || cars.get() key=|c| c.id.clone()
                                children=move |c| view! { <option value=c.id.clone()>{c.name.clone()}</option> } />
                        </select>
                    </label>
                    <label class="garage-check">
                        <input type="checkbox" prop:checked=move || notify.get()
                            on:change=move |ev| notify.set(event_target_checked(&ev)) />
                        <span>"Notify on arrive / leave"</span>
                    </label>
                    <div class="row">
                        <button type="button" class="btn primary btn-sm"
                            prop:disabled=move || busy.get() || name.get().trim().is_empty() || !draft_ready()
                            on:click=save>
                            <Icon name="floppy-disk" size=IconSize::Sm />
                            {move || if editing.get().is_some() { "Save changes" } else { "Save place" }}
                        </button>
                        <Show when=move || editing.get().is_some() || draft_ready()>
                            <button type="button" class="btn ghost btn-sm" on:click=move |_| clear_draft()>"Cancel"</button>
                        </Show>
                    </div>
                </div>
            </section>

            <section class="card places-list-card">
                <h2 class="section-title">
                    <Icon name="list-bullets" color=IconColor::Accent />
                    "Your places"
                </h2>
                <Show
                    when=move || !places.get().is_empty()
                    fallback=|| view! { <p class="muted">"No places yet — draw one on the map."</p> }
                >
                    <ul class="places-list">
                        <For
                            each=move || places.get()
                            key=|g| format!("{}:{}:{}", g.id, g.notify, g.name)
                            children=move |g| {
                                let g_edit = g.clone();
                                let (id_del, id_notify, id_events) = (g.id.clone(), g.id.clone(), g.id.clone());
                                let id_open = g.id.clone();
                                let del_name = g.name.clone();
                                let notify_now = g.notify;
                                let car_label = car_name(&g.car_id);
                                view! {
                                    <li class="place-item">
                                        <div class="place-main">
                                            <span class="place-name">{g.name.clone()}</span>
                                            <span class="muted place-meta">{format!("{} · {car_label}", shape_label(&g))}</span>
                                        </div>
                                        <div class="place-actions">
                                            <label class="trip-select-toggle" title="Notify on arrive / leave">
                                                <input type="checkbox" prop:checked=notify_now
                                                    on:change=move |ev| {
                                                        let on = event_target_checked(&ev);
                                                        let id = id_notify.clone();
                                                        leptos::task::spawn_local(async move {
                                                            match update_geofence(&id, &serde_json::json!({ "notify": on })).await {
                                                                Ok(_) => refresh.update(|n| *n = n.wrapping_add(1)),
                                                                Err(e) => { let _ = error.try_set(Some(e.to_string())); }
                                                            }
                                                        });
                                                    } />
                                                <Icon name="bell" size=IconSize::Sm />
                                            </label>
                                            <button type="button" class="btn ghost btn-sm"
                                                on:click=move |_| {
                                                    let id = id_events.clone();
                                                    if open_events.get_untracked().as_deref() == Some(id.as_str()) {
                                                        open_events.set(None);
                                                        return;
                                                    }
                                                    open_events.set(Some(id.clone()));
                                                    events.set(Vec::new());
                                                    leptos::task::spawn_local(async move {
                                                        match geofence_events(&id).await {
                                                            Ok(e) => { let _ = events.try_set(e); }
                                                            Err(e) => { let _ = error.try_set(Some(e.to_string())); }
                                                        }
                                                    });
                                                }>
                                                "Events"
                                            </button>
                                            <button type="button" class="btn ghost btn-sm"
                                                on:click=move |_| {
                                                    let g = g_edit.clone();
                                                    editing.set(Some(g.id.clone()));
                                                    name.set(g.name.clone());
                                                    notify.set(g.notify);
                                                    car_id.set(g.car_id.clone().unwrap_or_default());
                                                    match (&g.polygon, g.center_lat, g.center_lon, g.radius_m) {
                                                        (Some(p), _, _, _) if p.len() >= 3 => {
                                                            shape.set(Shape::Polygon);
                                                            vertices.set(p.clone());
                                                            center.set(None);
                                                        }
                                                        (_, Some(lat), Some(lon), Some(r)) => {
                                                            shape.set(Shape::Circle);
                                                            center.set(Some([lon, lat]));
                                                            radius.set(r.clamp(50.0, 5000.0));
                                                            vertices.set(Vec::new());
                                                        }
                                                        _ => {}
                                                    }
                                                }>
                                                <Icon name="pencil-simple" size=IconSize::Sm />
                                            </button>
                                            <button type="button" class="btn ghost btn-sm err"
                                                aria-label="Delete place"
                                                on:click=move |_| {
                                                    if !confirm(&format!("Delete “{del_name}”?")) {
                                                        return;
                                                    }
                                                    let id = id_del.clone();
                                                    leptos::task::spawn_local(async move {
                                                        match delete_geofence(&id).await {
                                                            Ok(()) => refresh.update(|n| *n = n.wrapping_add(1)),
                                                            Err(e) => { let _ = error.try_set(Some(e.to_string())); }
                                                        }
                                                    });
                                                }>
                                                <Icon name="trash" size=IconSize::Sm />
                                            </button>
                                        </div>
                                        <Show when=move || open_events.get().as_deref() == Some(id_open.as_str())>
                                            <ul class="place-events">
                                                {move || {
                                                    let list = events.get();
                                                    if list.is_empty() {
                                                        return view! { <li class="muted">"No arrivals or departures recorded yet."</li> }.into_any();
                                                    }
                                                    list.into_iter()
                                                        .take(20)
                                                        .map(|e| {
                                                            let when = chrono::DateTime::parse_from_rfc3339(e.at.trim())
                                                                .map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
                                                                .unwrap_or_else(|_| e.at.clone());
                                                            let verb = if e.kind == "enter" { "Arrived" } else { "Left" };
                                                            let who = car_name(&Some(e.car_id.clone()));
                                                            view! {
                                                                <li>
                                                                    <span class=if e.kind == "enter" { "pill pill-ok" } else { "pill" }>{verb}</span>
                                                                    <span>{format!("{who} · {when}")}</span>
                                                                    {e.track_id.clone().map(|t| view! {
                                                                        <A href=format!("/app/trips/{t}")>"Trip"</A>
                                                                    })}
                                                                </li>
                                                            }
                                                        })
                                                        .collect_view()
                                                        .into_any()
                                                }}
                                            </ul>
                                        </Show>
                                    </li>
                                }
                            }
                        />
                    </ul>
                </Show>
            </section>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circle_ring_stays_at_the_radius() {
        let ring = circle_ring(40.0, -3.7, 500.0);
        assert_eq!(ring.len(), 64);
        let [lon, lat] = ring[16]; // due east
        let dx = (lon + 3.7).to_radians() * 6_371_000.0 * 40f64.to_radians().cos();
        assert!((dx - 500.0).abs() < 5.0, "{dx}");
        assert!((lat - 40.0).abs() < 1e-3);
    }
}
