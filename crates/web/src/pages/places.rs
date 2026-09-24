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
use crate::components::geo::{AreasData, AreasMap, MapArea, circle_ring, ring_label_point};
use crate::components::{Icon, IconColor, IconSize};

const PLACES_MAP_ID: &str = "places-map";
/// Radius slider range for new places (the server accepts 10 m to 100 km;
/// editing a place outside this range widens the slider instead of clamping).
const RADIUS_MIN_M: f64 = 50.0;
const RADIUS_MAX_M: f64 = 5_000.0;

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

/// Outline of a stored place and where its label goes.
fn ring_of(g: &Geofence) -> (Vec<[f64; 2]>, Option<[f64; 2]>) {
    if let Some(p) = g.polygon.as_ref().filter(|p| p.len() >= 3) {
        return (p.clone(), ring_label_point(p));
    }
    match (g.center_lat, g.center_lon, g.radius_m) {
        (Some(lat), Some(lon), Some(r)) => (circle_ring(lat, lon, r), Some([lon, lat])),
        _ => (Vec::new(), None),
    }
}

/// `575 m`, `1,5 km` (locale decimals).
fn radius_text(r: f64) -> String {
    use crate::i18n::num;
    if r >= 1000.0 {
        format!("{} km", num(r / 1000.0, 1))
    } else {
        format!("{} m", num(r, 0))
    }
}

fn shape_label(g: &Geofence) -> String {
    use crate::i18n::tf;
    match (&g.polygon, g.radius_m) {
        (Some(p), _) if p.len() >= 3 => tf("places.area_points", &[("n", &p.len())]),
        (_, Some(r)) => tf("places.circle", &[("r", &radius_text(r))]),
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
    // First list response arrived (so "no places" means none, not "loading").
    let loaded = RwSignal::new(false);

    // Draft shape.
    let shape = RwSignal::new(Shape::Circle);
    let center = RwSignal::new(Option::<[f64; 2]>::None);
    let radius = RwSignal::new(200.0f64);
    // Slider bounds; widened while editing a place outside the default range.
    let radius_bounds = RwSignal::new((RADIUS_MIN_M, RADIUS_MAX_M));
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
                    let _ = loaded.try_set(true);
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
        radius_bounds.set((RADIUS_MIN_M, RADIUS_MAX_M));
        radius.update(|r| *r = r.clamp(RADIUS_MIN_M, RADIUS_MAX_M));
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
                .map(|g| {
                    let (ring, label_at) = ring_of(g);
                    MapArea {
                        id: g.id.clone(),
                        ring,
                        draft: false,
                        label: g.name.clone(),
                        label_at,
                    }
                })
                .collect()
        });
        let ring = draft_ring();
        if ring.len() >= 3 {
            let label_at = match shape.get() {
                Shape::Circle => center.get(),
                Shape::Polygon => ring_label_point(&ring),
            };
            areas.push(MapArea {
                id: "draft".into(),
                ring,
                draft: true,
                label: name.get(),
                label_at,
            });
        }
        // Frame the saved places once they load and whenever one is added or
        // removed; editing frames the place being edited.
        let fit_key = if loaded.get() {
            places.with(|p| {
                let mut ids: Vec<&str> = p.iter().map(|g| g.id.as_str()).collect();
                ids.sort_unstable();
                format!("places:{}", ids.join(","))
            })
        } else {
            String::new()
        };
        AreasData {
            areas,
            vertices: if shape.get() == Shape::Polygon {
                vertices.get()
            } else {
                Vec::new()
            },
            fit_key,
            focus_key: edit.map(|id| format!("edit:{id}")).unwrap_or_default(),
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
                    let _ =
                        notice.try_set(Some(crate::i18n::tf("places.saved", &[("name", &g.name)])));
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
        None => crate::i18n::t("common.all_cars").to_string(),
        Some(id) => cars.with(|c| {
            c.iter()
                .find(|c| &c.id == id)
                .map(|c| c.name.clone())
                .unwrap_or_else(|| crate::i18n::t("places.a_car").into())
        }),
    };

    view! {
        <div class="topbar">
            <div>
                <h1 class="section-title">
                    <Icon name="map-pin-area" color=IconColor::Accent />
                    {tr!("nav.places")}
                </h1>
                <p class="muted">{tr!("places.lead")}</p>
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
                    <div class="seg-control" role="group" aria-label=tr!("places.shape")>
                        <button type="button"
                            class=move || if shape.get() == Shape::Circle { "seg-btn is-active" } else { "seg-btn" }
                            aria-pressed=move || (shape.get() == Shape::Circle).to_string()
                            on:click=move |_| shape.set(Shape::Circle)>{tr!("places.circle_btn")}</button>
                        <button type="button"
                            class=move || if shape.get() == Shape::Polygon { "seg-btn is-active" } else { "seg-btn" }
                            aria-pressed=move || (shape.get() == Shape::Polygon).to_string()
                            on:click=move |_| shape.set(Shape::Polygon)>{tr!("places.polygon")}</button>
                    </div>
                    <Show
                        when=move || shape.get() == Shape::Circle
                        fallback=move || view! {
                            <span class="muted">{move || crate::i18n::tf("places.points_hint", &[("n", &vertices.get().len())])}</span>
                            <button type="button" class="btn ghost btn-sm"
                                prop:disabled=move || vertices.get().is_empty()
                                on:click=move |_| vertices.update(|v| { v.pop(); })>{tr!("places.undo")}</button>
                            <button type="button" class="btn ghost btn-sm"
                                prop:disabled=move || vertices.get().is_empty()
                                on:click=move |_| vertices.set(Vec::new())>{tr!("common.clear")}</button>
                        }
                    >
                        <label class="places-radius">
                            <span class="muted">{move || if center.get().is_some() { crate::i18n::t("places.radius") } else { crate::i18n::t("places.place_center") }}</span>
                            <input type="range" step="25"
                                min=move || radius_bounds.get().0.to_string()
                                max=move || radius_bounds.get().1.to_string()
                                prop:value=move || radius.get().to_string()
                                on:input=move |ev| {
                                    if let Ok(v) = event_target_value(&ev).parse::<f64>() {
                                        radius.set(v);
                                    }
                                } />
                            <span class="places-radius-value">{move || radius_text(radius.get())}</span>
                        </label>
                    </Show>
                </div>
                <AreasMap id=PLACES_MAP_ID data=map_data />
                <div class="places-form">
                    <label class="garage-field">
                        <span>{tr!("common.name")}</span>
                        <input type="text" maxlength="80" placeholder=tr!("places.home")
                            prop:value=move || name.get()
                            on:input=move |ev| name.set(event_target_value(&ev)) />
                    </label>
                    <label class="garage-field">
                        <span>{tr!("common.car")}</span>
                        <select prop:value=move || { cars.track(); car_id.get() }
                            prop:disabled=move || editing.get().is_some()
                            on:change=move |ev| car_id.set(event_target_value(&ev))>
                            <option value="">{tr!("common.all_cars")}</option>
                            <For each=move || cars.get() key=|c| c.id.clone()
                                children=move |c| view! { <option value=c.id.clone()>{c.name.clone()}</option> } />
                        </select>
                    </label>
                    <label class="garage-check">
                        <input type="checkbox" prop:checked=move || notify.get()
                            on:change=move |ev| notify.set(event_target_checked(&ev)) />
                        <span>{tr!("places.notify")}</span>
                    </label>
                    <div class="row">
                        <button type="button" class="btn primary btn-sm"
                            prop:disabled=move || busy.get() || name.get().trim().is_empty() || !draft_ready()
                            on:click=save>
                            <Icon name="floppy-disk" size=IconSize::Sm />
                            {move || if editing.get().is_some() { crate::i18n::t("places.save_changes") } else { crate::i18n::t("places.save_place") }}
                        </button>
                        <Show when=move || editing.get().is_some() || draft_ready()>
                            <button type="button" class="btn ghost btn-sm" on:click=move |_| clear_draft()>{tr!("common.cancel")}</button>
                        </Show>
                    </div>
                </div>
            </section>

            <section class="card places-list-card">
                <h2 class="section-title">
                    <Icon name="list-bullets" color=IconColor::Accent />
                    {tr!("places.your_places")}
                </h2>
                <Show
                    when=move || !places.with(Vec::is_empty)
                    fallback=move || {
                        if loaded.get() {
                            view! { <p class="muted">{tr!("places.none")}</p> }.into_any()
                        } else if error.with(Option::is_some) {
                            ().into_any()
                        } else {
                            view! { <p class="muted" role="status">{tr!("common.loading")}</p> }.into_any()
                        }
                    }
                >
                    <ul class="places-list">
                        <For
                            each=move || places.get()
                            // Any change (name, shape, radius, car) re-renders the row.
                            key=|g| format!("{g:?}")
                            children=move |g| {
                                let g_edit = g.clone();
                                let (id_del, id_notify, id_events) = (g.id.clone(), g.id.clone(), g.id.clone());
                                let (id_open, id_checked) = (g.id.clone(), g.id.clone());
                                let del_name = g.name.clone();
                                // Follows the list, so a failed toggle snaps back after the refresh.
                                let notify_now = move || places.with(|p| {
                                    p.iter().any(|x| x.id == id_checked && x.notify)
                                });
                                let car_of = g.car_id.clone();
                                let g_label = g.clone();
                                let meta = move || format!("{} · {}", shape_label(&g_label), car_name(&car_of));
                                view! {
                                    <li class="place-item">
                                        <div class="place-main">
                                            <span class="place-name">{g.name.clone()}</span>
                                            <span class="muted place-meta">{meta}</span>
                                        </div>
                                        <div class="place-actions">
                                            <label class="trip-select-toggle" title=tr!("places.notify")>
                                                <input type="checkbox" prop:checked=notify_now
                                                    on:change=move |ev| {
                                                        let on = event_target_checked(&ev);
                                                        let id = id_notify.clone();
                                                        leptos::task::spawn_local(async move {
                                                            match update_geofence(&id, &serde_json::json!({ "notify": on })).await {
                                                                Ok(_) => {
                                                                    // Keep an open edit of this place in step.
                                                                    if editing.get_untracked().as_deref() == Some(id.as_str()) {
                                                                        let _ = notify.try_set(on);
                                                                    }
                                                                }
                                                                Err(e) => { let _ = error.try_set(Some(e.to_string())); }
                                                            }
                                                            refresh.update(|n| *n = n.wrapping_add(1));
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
                                                {tr!("places.events")}
                                            </button>
                                            <button type="button" class="btn ghost btn-sm"
                                                on:click=move |_| {
                                                    let g = g_edit.clone();
                                                    notice.set(None);
                                                    error.set(None);
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
                                                            // Widen the slider rather than silently
                                                            // resizing the place on save.
                                                            radius_bounds.set((r.min(RADIUS_MIN_M), r.max(RADIUS_MAX_M)));
                                                            radius.set(r);
                                                            vertices.set(Vec::new());
                                                        }
                                                        _ => {}
                                                    }
                                                }>
                                                <Icon name="pencil-simple" size=IconSize::Sm />
                                            </button>
                                            <button type="button" class="btn ghost btn-sm err"
                                                aria-label=tr!("places.delete_place")
                                                on:click=move |_| {
                                                    if !confirm(&crate::i18n::tf("places.confirm_delete", &[("name", &del_name)])) {
                                                        return;
                                                    }
                                                    let id = id_del.clone();
                                                    leptos::task::spawn_local(async move {
                                                        match delete_geofence(&id).await {
                                                            Ok(()) => {
                                                                // Don't leave the form editing a place that is gone.
                                                                if editing.get_untracked().as_deref() == Some(id.as_str()) {
                                                                    clear_draft();
                                                                }
                                                                if open_events.get_untracked().as_deref() == Some(id.as_str()) {
                                                                    open_events.set(None);
                                                                }
                                                                refresh.update(|n| *n = n.wrapping_add(1));
                                                            }
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
                                                        return view! { <li class="muted">{tr!("places.no_events")}</li> }.into_any();
                                                    }
                                                    list.into_iter()
                                                        .take(20)
                                                        .map(|e| {
                                                            let when = crate::i18n::local_datetime(&e.at, None);
                                                            let verb = if e.kind == "enter" {
                                                                crate::i18n::t("places.arrived")
                                                            } else {
                                                                crate::i18n::t("places.left")
                                                            };
                                                            let who = car_name(&Some(e.car_id.clone()));
                                                            view! {
                                                                <li>
                                                                    <span class=if e.kind == "enter" { "pill pill-ok" } else { "pill" }>{verb}</span>
                                                                    <span>{format!("{who} · {when}")}</span>
                                                                    {e.track_id.clone().map(|t| view! {
                                                                        <A href=format!("/app/trips/{t}")>{tr!("trip.title")}</A>
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
