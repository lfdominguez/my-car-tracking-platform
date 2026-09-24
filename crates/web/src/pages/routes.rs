//! Routes Optimization SPA pages.

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;

use crate::api::{
    Car, RouteCorridorDetail, RouteOptSummary, list_cars, route_opt_corridor,
    route_opt_corridor_map, route_opt_recompute, route_opt_summary,
};
use crate::components::{Icon, IconColor, IconSize};
use crate::i18n::{t, tf, tp};
use crate::units::{fmt_distance, use_unit_prefs};

fn fmt_duration(secs: f64) -> String {
    let m = (secs / 60.0).round().max(0.0) as i64;
    if m < 60 {
        format!("{m} min")
    } else {
        format!("{}h {}m", m / 60, m % 60)
    }
}

/// Must match `VARIANT_COLORS` in map.rs corridor map.
const VARIANT_SWATCHES: &[&str] = &[
    "#0077ff", "#00c853", "#ffd600", "#00e5ff", "#304ffe", "#76ff03",
];
/// Must match `ORS_COLORS` in map.rs corridor map.
const ORS_SWATCHES: &[&str] = &["#ff2d95", "#ff9100", "#d500f9", "#ff1744"];

fn variant_swatch(i: usize) -> &'static str {
    VARIANT_SWATCHES[i % VARIANT_SWATCHES.len()]
}

fn ors_swatch(i: usize) -> &'static str {
    ORS_SWATCHES[i % ORS_SWATCHES.len()]
}

/// Stamp each map line with the color of its list row, matched by identity
/// (`variant_id`, ORS `preference`) rather than by the order the map endpoint
/// happened to return, so a swatch in the list always names its line. Lines
/// with no row keep the map's own index-based color.
fn stamp_corridor_colors(
    geo: &mut serde_json::Value,
    variant_ids: &[String],
    ors_prefs: &[String],
) {
    let Some(features) = geo
        .get_mut("features")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };
    for f in features {
        let Some(props) = f
            .get_mut("properties")
            .and_then(serde_json::Value::as_object_mut)
        else {
            continue;
        };
        let (key, rows, swatch): (_, _, fn(usize) -> &'static str) =
            match props.get("kind").and_then(|k| k.as_str()) {
                Some("variant") => ("variant_id", variant_ids, variant_swatch),
                Some("ors") => ("preference", ors_prefs, ors_swatch),
                _ => continue,
            };
        let Some(idx) = props
            .get(key)
            .and_then(|v| v.as_str())
            .and_then(|id| rows.iter().position(|r| r == id))
        else {
            continue;
        };
        props.insert("color_index".into(), idx.into());
        props.insert("route_color".into(), swatch(idx).into());
    }
}

/// `[min_lon, min_lat, max_lon, max_lat]` of the variant line `variant_id`, for
/// zooming the map onto it. `None` when it has no drawable coordinates.
fn variant_bounds(geo: &serde_json::Value, variant_id: &str) -> Option<[f64; 4]> {
    let feature = geo.get("features")?.as_array()?.iter().find(|f| {
        let props = &f["properties"];
        props["kind"] == "variant" && props["variant_id"].as_str() == Some(variant_id)
    })?;
    feature["geometry"]["coordinates"]
        .as_array()?
        .iter()
        .filter_map(|c| Some((c.get(0)?.as_f64()?, c.get(1)?.as_f64()?)))
        .filter(|(lon, lat)| lon.is_finite() && lat.is_finite())
        .fold(None, |acc: Option<[f64; 4]>, (lon, lat)| {
            Some(match acc {
                None => [lon, lat, lon, lat],
                Some([x0, y0, x1, y1]) => [x0.min(lon), y0.min(lat), x1.max(lon), y1.max(lat)],
            })
        })
}

/// Clicking the selected variant again clears the selection.
fn toggle_selection(current: Option<&str>, clicked: &str) -> Option<String> {
    (current != Some(clicked)).then(|| clicked.to_owned())
}

/// Human label for insight `kind` codes from the server.
fn insight_kind_label(kind: &str) -> &'static str {
    t(match kind {
        "prefer_variant" | "prefer_variant_soft" => "routes.faster_path",
        "avoid_variant_now" | "avoid_variant_now_soft" => "routes.right_now",
        "ors_reference" => "routes.router_tip",
        "ors_matches" => "routes.matches_router",
        "beats_router" => "routes.beats_router",
        "forming" => "routes.forming",
        "typical_pace" => "routes.baseline",
        "single_path" => "routes.one_path",
        "time_window" => "routes.this_hour",
        "peak_vs_offpeak" => "routes.peak_hours",
        "weekend_vs_weekday" => "routes.weekend",
        "high_stops" => "routes.stops",
        _ => "routes.insight",
    })
}

fn insight_kind_class(kind: &str) -> &'static str {
    match kind {
        "prefer_variant" | "beats_router" => "is-positive",
        "prefer_variant_soft" | "ors_matches" | "typical_pace" => "is-neutral",
        "avoid_variant_now" | "avoid_variant_now_soft" | "ors_reference" | "high_stops" => {
            "is-warn"
        }
        "forming" | "single_path" => "is-muted",
        _ => "is-neutral",
    }
}

#[component]
pub fn RoutesPage() -> impl IntoView {
    let cars = RwSignal::new(Vec::<Car>::new());
    let car_id = RwSignal::new(String::new());
    let summary = RwSignal::new(Option::<RouteOptSummary>::None);
    let error = RwSignal::new(Option::<String>::None);
    let busy = RwSignal::new(false);
    let message = RwSignal::new(Option::<String>::None);
    let prefs = use_unit_prefs();

    Effect::new(move |_| {
        leptos::task::spawn_local(async move {
            match list_cars().await {
                Ok(list) => {
                    if car_id.get_untracked().is_empty()
                        && let Some(c) = list.first()
                    {
                        car_id.set(c.id.clone());
                    }
                    cars.set(list);
                }
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    });

    // Bumped per car switch: a slow summary for the previous car must not land on
    // top of the one now selected.
    let fetch_gen = RwSignal::new(0u64);
    Effect::new(move |_| {
        let id = car_id.get();
        if id.is_empty() {
            return;
        }
        let req = fetch_gen.get_untracked().wrapping_add(1);
        fetch_gen.set(req);
        summary.set(None);
        message.set(None);
        error.set(None);
        leptos::task::spawn_local(async move {
            let result = route_opt_summary(&id).await;
            if fetch_gen.try_get_untracked() != Some(req) {
                return;
            }
            match result {
                Ok(s) => summary.set(Some(s)),
                Err(e) => {
                    summary.set(None);
                    error.set(Some(e.to_string()));
                }
            }
        });
    });

    let recompute = move |_| {
        let id = car_id.get_untracked();
        if id.is_empty() {
            return;
        }
        busy.set(true);
        message.set(None);
        error.set(None);
        leptos::task::spawn_local(async move {
            let still_selected = || car_id.try_get_untracked().as_deref() == Some(id.as_str());
            match route_opt_recompute(&id).await {
                Ok(r) if still_selected() => {
                    message.set(Some(tf("routes.recomputed", &[("n", &r.processed)])));
                    if let Ok(s) = route_opt_summary(&id).await
                        && still_selected()
                    {
                        summary.set(Some(s));
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    if still_selected() {
                        error.set(Some(e.to_string()));
                    }
                }
            }
            busy.set(false);
        });
    };

    view! {
        <div class="page-header">
            <h1>{tr!("nav.routes")}</h1>
            <p class="muted">{tr!("routes.lead")}</p>
        </div>

        <Show when=move || message.get().is_some()>
            <div class="banner ok">{move || message.get().unwrap_or_default()}</div>
        </Show>
        <Show when=move || error.get().is_some()>
            <div class="banner err">{move || error.get().unwrap_or_default()}</div>
        </Show>

        <div class="card routes-toolbar">
            <div class="form-row" style="margin:0">
                <label>{tr!("common.car")}</label>
                <select
                    prop:value=move || car_id.get()
                    on:change=move |ev| car_id.set(event_target_value(&ev))
                >
                    <For
                        each=move || cars.get()
                        key=|c| c.id.clone()
                        children=move |c| {
                            view! { <option value=c.id.clone()>{c.name.clone()}</option> }
                        }
                    />
                </select>
            </div>
            <button class="btn secondary" disabled=move || busy.get() || car_id.get().is_empty() on:click=recompute>
                <Icon name="arrows-clockwise" size=IconSize::Sm />
                {move || if busy.get() { t("routes.recomputing") } else { t("routes.recompute") }}
            </button>
        </div>

        {move || {
            let s = summary.get();
            let Some(s) = s else {
                return view! { <p class="muted">{tr!("routes.select_car")}</p> }.into_any();
            };
            if !s.ors_configured {
                view! {
                    <div class="banner warn">
                        {tr!("routes.ors_missing")}
                        " "
                        <A href="/app/settings">{tr!("routes.add_in_settings")}</A>
                        " "
                        {tr!("routes.ors_missing_tail")}
                    </div>
                }.into_any()
            } else {
                ().into_any()
            }
        }}

        <h2 class="section-title" style="margin-top:1.25rem">
            <Icon name="map-trifold" color=IconColor::Accent />
            {tr!("routes.corridors")}
        </h2>
        {move || {
            let s = summary.get();
            let corridors = s.map(|s| s.corridors).unwrap_or_default();
            if corridors.is_empty() {
                return view! {
                    <p class="muted">{tr!("routes.no_corridors")}</p>
                }.into_any();
            }
            view! {
                <div class="routes-corridor-grid">
                    <For
                        each=move || summary.get().map(|s| s.corridors).unwrap_or_default()
                        key=|c| c.id.clone()
                        children=move |c| {
                            let id = c.id.clone();
                            let forming = c.forming;
                            let round_trip = c.is_round_trip;
                            let best = c.best_variant_label.clone().unwrap_or_else(|| "—".into());
                            let dur = c.median_duration_secs.map(fmt_duration).unwrap_or_else(|| "—".into());
                            // Reactive: rows can render before `/api/me` settles the units.
                            let median_distance = c.median_distance;
                            let dist = move || {
                                median_distance
                                    .map(|d| fmt_distance(Some(d), &prefs.get()))
                                    .unwrap_or_else(|| "—".into())
                            };
                            // Coordinates stay in the machine `lat, lon` form in every
                            // language: a decimal comma would collide with the separator.
                            let (via, base) = (
                                c.via_lat.zip(c.via_lon).map(|(a, b)| format!("{a:.4}, {b:.4}")),
                                format!("{:.4}, {:.4}", c.start_lat, c.start_lon),
                            );
                            let od_label = move || if round_trip {
                                match &via {
                                    Some(via) => tf("routes.round_trip_via", &[("via", via), ("base", &base)]),
                                    None => tf("routes.round_trip_base", &[("base", &base)]),
                                }
                            } else {
                                format!(
                                    "{:.4}, {:.4} → {:.4}, {:.4}",
                                    c.start_lat, c.start_lon, c.end_lat, c.end_lon
                                )
                            };
                            view! {
                                <A href=format!("/app/routes/{id}")>
                                    <div class="card routes-corridor-card">
                                        <div class="routes-corridor-top">
                                            <strong>{move || tp("common.trips_count", c.trip_count as i64)}</strong>
                                            <div class="routes-pill-row">
                                                {if round_trip {
                                                    view! { <span class="pill">{tr!("routes.round_trip")}</span> }.into_any()
                                                } else {
                                                    view! { <span></span> }.into_any()
                                                }}
                                                {if forming {
                                                    view! { <span class="pill warn">{tr!("routes.forming")}</span> }.into_any()
                                                } else {
                                                    view! { <span class="pill ok">{tr!("status.ready")}</span> }.into_any()
                                                }}
                                            </div>
                                        </div>
                                        <div class="muted" style="font-size:var(--text-sm)">
                                            {od_label}
                                        </div>
                                        <div class="routes-corridor-metrics">
                                            <span>{tr!("routes.best")}" "{best}</span>
                                            <span>{tr!("routes.median")}" "{dur}</span>
                                            <span>{tr!("routes.dist")}" "{dist}</span>
                                        </div>
                                    </div>
                                </A>
                            }
                        }
                    />
                </div>
            }.into_any()
        }}
    }
}

#[component]
pub fn RouteCorridorPage() -> impl IntoView {
    let params = use_params_map();
    let detail = RwSignal::new(Option::<RouteCorridorDetail>::None);
    let map_geo = RwSignal::new(Option::<serde_json::Value>::None);
    let error = RwSignal::new(Option::<String>::None);
    let prefs = use_unit_prefs();
    let map_host = NodeRef::<leptos::html::Div>::new();
    // Variant picked in the list or on the map (sticky), and the one previewed
    // while a row is hovered or focused.
    let selected = RwSignal::new(Option::<String>::None);
    let hovered = RwSignal::new(Option::<String>::None);

    // The page is reused when only `:id` changes: reset what belongs to the old
    // corridor and drop responses that arrive after the next navigation.
    let fetch_gen = RwSignal::new(0u64);
    Effect::new(move |_| {
        let id = params.with(|p| p.get("id").unwrap_or_default());
        if id.is_empty() {
            return;
        }
        let req = fetch_gen.get_untracked().wrapping_add(1);
        fetch_gen.set(req);
        detail.set(None);
        map_geo.set(None);
        error.set(None);
        selected.set(None);
        hovered.set(None);
        let current = move || fetch_gen.try_get_untracked() == Some(req);
        leptos::task::spawn_local(async move {
            let corridor = route_opt_corridor(&id).await;
            if !current() {
                return;
            }
            match corridor {
                Ok(d) => detail.set(Some(d)),
                Err(e) => error.set(Some(e.to_string())),
            }
            let geo = route_opt_corridor_map(&id).await;
            if !current() {
                return;
            }
            map_geo.set(geo.ok());
        });
    });

    Effect::new(move |_| {
        let geo = map_geo.get();
        let Some(geo) = geo else {
            // Between corridors: don't leave the previous one's lines on the map.
            crate::components::map::dispose_route_opt_map();
            return;
        };
        let Some(el) = map_host.get() else { return };
        let (variant_ids, ors_prefs) = detail.with(|d| {
            d.as_ref()
                .map(|d| {
                    (
                        d.variants.iter().map(|v| v.id.clone()).collect::<Vec<_>>(),
                        d.ors_alternatives
                            .iter()
                            .map(|a| a.preference.clone())
                            .collect::<Vec<_>>(),
                    )
                })
                .unwrap_or_default()
        });
        let mut geo = geo;
        stamp_corridor_colors(&mut geo, &variant_ids, &ors_prefs);
        crate::components::map::mount_route_opt_map(&el, &geo);
        // A fresh map starts unhighlighted; re-apply whatever is picked.
        crate::components::map::set_route_opt_selection(
            selected.get_untracked().as_deref(),
            hovered.get_untracked().as_deref(),
            None,
        );
    });

    Effect::new(move |_| {
        let sel = selected.get();
        let hov = hovered.get();
        let bounds = sel.as_deref().and_then(|id| {
            map_geo.with_untracked(|g| g.as_ref().and_then(|g| variant_bounds(g, id)))
        });
        crate::components::map::set_route_opt_selection(sel.as_deref(), hov.as_deref(), bounds);
    });

    // Clicks on a variant line in the map (see `ROUTE_OPT_VARIANT_SELECT_EVENT`).
    Effect::new(move |_| {
        let handle = window_event_listener_untyped(
            crate::components::map::ROUTE_OPT_VARIANT_SELECT_EVENT,
            move |ev| {
                let id = js_sys::Reflect::get(&ev, &"detail".into())
                    .and_then(|d| js_sys::Reflect::get(&d, &"id".into()))
                    .ok()
                    .and_then(|v| v.as_string());
                if let Some(id) = id {
                    let _ = selected.try_update(|s| *s = toggle_selection(s.as_deref(), &id));
                }
            },
        );
        on_cleanup(move || handle.remove());
    });

    on_cleanup(move || {
        crate::components::map::dispose_route_opt_map();
    });

    let toggle_variant = move |id: &str| {
        selected.update(|s| *s = toggle_selection(s.as_deref(), id));
    };
    let is_selected = move |id: &str| selected.with(|s| s.as_deref() == Some(id));
    let show_all = move || {
        view! {
            <Show when=move || selected.with(Option::is_some)>
                <button
                    type="button"
                    class="btn ghost btn-sm routes-show-all"
                    on:click=move |_| selected.set(None)
                >
                    <Icon name="arrows-out" size=IconSize::Sm />
                    {tr!("routes.show_all")}
                </button>
            </Show>
        }
    };

    view! {
        <div class="page-header">
            <div>
                <A href="/app/routes"><span class="muted">{tr!("routes.back")}</span></A>
                <h1>{tr!("routes.corridor")}</h1>
            </div>
        </div>
        <Show when=move || error.get().is_some()>
            <div class="banner err">{move || error.get().unwrap_or_default()}</div>
        </Show>

        {move || {
            let d = detail.get();
            let Some(d) = d else {
                return view! { <p class="muted">{tr!("common.loading")}</p> }.into_any();
            };
            let rec = d.recommendation_for_now.clone();
            let round_trip = d.is_round_trip;
            let via_note = if round_trip {
                if let (Some(vlat), Some(vlon)) = (d.via_lat, d.via_lon) {
                    tf("routes.round_trip_corridor_via", &[("via", &format!("{vlat:.4}, {vlon:.4}"))])
                } else {
                    t("routes.round_trip_corridor").into()
                }
            } else {
                format!(
                    "{:.4}, {:.4} → {:.4}, {:.4}",
                    d.start_lat, d.start_lon, d.end_lat, d.end_lon
                )
            };
            view! {
                <div class="card routes-rec">
                    <Icon name="compass" color=IconColor::Accent />
                    <div>
                        <strong>
                            {rec.variant_label.clone().unwrap_or_else(|| t("routes.no_recommendation").into())}
                        </strong>
                        <p class="muted" style="margin:0.25rem 0 0">{rec.reason.clone()}</p>
                        <p class="muted" style="margin:0.35rem 0 0;font-size:var(--text-sm)">{via_note}</p>
                    </div>
                    <div class="routes-pill-row">
                        {if round_trip {
                            view! { <span class="pill">{tr!("routes.round_trip")}</span> }.into_any()
                        } else {
                            view! { <span></span> }.into_any()
                        }}
                        {if d.forming {
                            view! { <span class="pill warn">{tr!("routes.forming_need_more")}</span> }.into_any()
                        } else {
                            view! { <span class="pill ok">{tp("common.trips_count", d.trip_count as i64)}</span> }.into_any()
                        }}
                    </div>
                </div>

                <div class="card" style="padding:0;overflow:hidden;margin-top:1rem">
                    <div class="routes-opt-map" node_ref=map_host></div>
                    <div class="routes-map-legend">
                        <div class="routes-map-legend-group">
                            <div class="routes-map-legend-title">
                                <span class="routes-line-sample is-variant"></span>
                                {tr!("routes.your_variants")}
                                <span class="muted">{tr!("routes.solid")}</span>
                                {show_all()}
                            </div>
                            <div class="routes-map-legend-items">
                                {d.variants.iter().enumerate().map(|(i, v)| {
                                    let color = variant_swatch(i).to_string();
                                    let label = v.label.clone();
                                    let id = v.id.clone();
                                    let (id_sel, id_pressed, id_click, id_in, id_out) =
                                        (id.clone(), id.clone(), id.clone(), id.clone(), id);
                                    let hint_label = label.clone();
                                    view! {
                                        <button
                                            type="button"
                                            class="routes-map-swatch-item routes-variant-chip"
                                            class:is-selected=move || is_selected(&id_sel)
                                            aria-pressed=move || is_selected(&id_pressed).to_string()
                                            title=move || tf("routes.highlight_variant", &[("label", &hint_label)])
                                            on:click=move |_| toggle_variant(&id_click)
                                            on:mouseenter=move |_| hovered.set(Some(id_in.clone()))
                                            on:mouseleave=move |_| {
                                                hovered.update(|h| if h.as_deref() == Some(id_out.as_str()) { *h = None });
                                            }
                                        >
                                            <span class="routes-map-swatch is-variant" style=format!("background:{color}")></span>
                                            {label}
                                        </button>
                                    }
                                }).collect_view()}
                            </div>
                        </div>
                        <div class="routes-map-legend-group">
                            <div class="routes-map-legend-title">
                                <span class="routes-line-sample is-ors"></span>
                                {tr!("routes.ors_alternatives")}
                                <span class="muted">{tr!("routes.dashed")}</span>
                            </div>
                            <div class="routes-map-legend-items">
                                {
                                    if d.ors_alternatives.is_empty() {
                                        view! { <span class="muted">{tr!("routes.none_cached")}</span> }.into_any()
                                    } else {
                                        d.ors_alternatives.iter().enumerate().map(|(i, a)| {
                                            let color = ors_swatch(i).to_string();
                                            let label = a.preference.clone();
                                            view! {
                                                <span class="routes-map-swatch-item">
                                                    <span class="routes-map-swatch is-ors" style=format!("background:{color}")></span>
                                                    {label}
                                                </span>
                                            }
                                        }).collect_view().into_any()
                                    }
                                }
                            </div>
                        </div>
                        <p class="muted routes-map-legend-hint">
                            {tr!("routes.legend_hint")}
                        </p>
                    </div>
                </div>

                <div class="routes-section-head">
                    <h2 class="section-title" style="margin-top:1.25rem">{tr!("routes.path_variants")}</h2>
                    {show_all()}
                </div>
                <div class="table-wrap">
                    <table
                        class="table routes-variant-table"
                        class:has-selection=move || selected.with(Option::is_some)
                    >
                        <caption class="sr-only">{tr!("routes.variants_caption")}</caption>
                        <thead>
                            <tr>
                                <th>{tr!("routes.variant")}</th>
                                <th>{tr!("nav.trips")}</th>
                                <th>{tr!("routes.median_time")}</th>
                                <th>{tr!("routes.median_distance")}</th>
                                <th>{tr!("routes.stops")}</th>
                                <th>{tr!("routes.elev_gain")}</th>
                            </tr>
                        </thead>
                        <tbody>
                            <For
                                each=move || {
                                    detail
                                        .get()
                                        .map(|d| {
                                            d.variants
                                                .into_iter()
                                                .enumerate()
                                                .collect::<Vec<_>>()
                                        })
                                        .unwrap_or_default()
                                }
                                key=|(_, v)| v.id.clone()
                                children=move |(i, v)| {
                                    let elev = v.median_elev_gain_m.map(|e| format!("{} m", crate::i18n::num(e, 0))).unwrap_or_else(|| "—".into());
                                    let median_distance = v.median_distance;
                                    let dist_label = move || fmt_distance(Some(median_distance), &prefs.get());
                                    let color = variant_swatch(i).to_string();
                                    let label = v.label.clone();
                                    let id = v.id.clone();
                                    let (id_sel, id_pressed, id_click) = (id.clone(), id.clone(), id.clone());
                                    let (id_in, id_out, id_focus, id_blur) =
                                        (id.clone(), id.clone(), id.clone(), id);
                                    let hint_label = label.clone();
                                    let preview = move |id: &str| hovered.set(Some(id.to_owned()));
                                    let unpreview = move |id: &str| {
                                        hovered.update(|h| if h.as_deref() == Some(id) { *h = None });
                                    };
                                    // The whole row is the click target; the name cell's
                                    // button gives keyboard users Enter / Space and the
                                    // pressed state (its click bubbles up to the row).
                                    view! {
                                        <tr
                                            class="routes-variant-row"
                                            class:is-selected=move || is_selected(&id_sel)
                                            on:click=move |_| toggle_variant(&id_click)
                                            on:mouseenter=move |_| preview(&id_in)
                                            on:mouseleave=move |_| unpreview(&id_out)
                                        >
                                            <td>
                                                <button
                                                    type="button"
                                                    class="routes-name-with-swatch routes-variant-toggle"
                                                    aria-pressed=move || is_selected(&id_pressed).to_string()
                                                    title=move || tf("routes.highlight_variant", &[("label", &hint_label)])
                                                    on:focus=move |_| preview(&id_focus)
                                                    on:blur=move |_| unpreview(&id_blur)
                                                >
                                                    <span class="routes-map-swatch is-variant" style=format!("background:{color}")></span>
                                                    {label}
                                                </button>
                                            </td>
                                            <td>{v.trip_count}</td>
                                            <td>{fmt_duration(v.median_duration_secs)}</td>
                                            <td>{dist_label}</td>
                                            <td>{fmt_duration(v.median_stop_time_secs)}</td>
                                            <td>{elev}</td>
                                        </tr>
                                    }
                                }
                            />
                        </tbody>
                    </table>
                </div>

                <Show when=move || detail.get().map(|d| !d.ors_alternatives.is_empty()).unwrap_or(false)>
                    <h2 class="section-title" style="margin-top:1.25rem">{tr!("routes.ors_alternatives")}</h2>
                    <div class="table-wrap">
                        <table class="table">
                            <thead>
                                <tr>
                                    <th>{tr!("routes.profile")}</th>
                                    <th>{tr!("routes.est_time")}</th>
                                    <th>{tr!("common.distance")}</th>
                                    <th>{tr!("routes.ascent")}</th>
                                    <th>{tr!("routes.descent")}</th>
                                </tr>
                            </thead>
                            <tbody>
                                <For
                                    each=move || {
                                        detail
                                            .get()
                                            .map(|d| {
                                                d.ors_alternatives
                                                    .into_iter()
                                                    .enumerate()
                                                    .collect::<Vec<_>>()
                                            })
                                            .unwrap_or_default()
                                    }
                                    key=|(i, a)| format!("{}-{}-{}", i, a.preference, a.fetched_at)
                                    children=move |(i, a)| {
                                        let distance = a.distance;
                                        let dist_label = move || fmt_distance(Some(distance), &prefs.get());
                                        let color = ors_swatch(i).to_string();
                                        let pref = a.preference.clone();
                                        view! {
                                            <tr>
                                                <td>
                                                    <span class="routes-name-with-swatch">
                                                        <span class="routes-map-swatch is-ors" style=format!("background:{color}")></span>
                                                        {pref}
                                                    </span>
                                                </td>
                                                <td>{fmt_duration(a.duration_secs)}</td>
                                                <td>{dist_label}</td>
                                                <td>{a.elev_gain_m.map(|e| format!("{} m", crate::i18n::num(e, 0))).unwrap_or_else(|| "—".into())}</td>
                                                <td>{a.elev_loss_m.map(|e| format!("{} m", crate::i18n::num(e, 0))).unwrap_or_else(|| "—".into())}</td>
                                            </tr>
                                        }
                                    }
                                />
                            </tbody>
                        </table>
                    </div>
                </Show>

                <h2 class="section-title" style="margin-top:1.25rem">
                    <Icon name="star" color=IconColor::Warn />
                    {tr!("routes.insights")}
                </h2>
                {move || {
                    let list = detail.get().map(|d| d.insights).unwrap_or_default();
                    if list.is_empty() {
                        return view! {
                            <div class="card routes-insight routes-insight-empty">
                                <p class="muted" style="margin:0">
                                    {tr!("routes.no_insights")}
                                </p>
                            </div>
                        }.into_any();
                    }
                    view! {
                        <div class="routes-insight-grid">
                            <For
                                each=move || detail.get().map(|d| d.insights).unwrap_or_default()
                                key=|i| i.id.clone()
                                children=move |i| {
                                    let kind = i.kind.clone();
                                    let kind_label = insight_kind_label(&kind).to_string();
                                    let kind_class = format!(
                                        "routes-insight-kind {}",
                                        insight_kind_class(&kind)
                                    );
                                    view! {
                                        <div class="card routes-insight">
                                            <div class=kind_class>{kind_label}</div>
                                            <h3>{i.title.clone()}</h3>
                                            <p class="muted" style="margin:0">{i.body.clone()}</p>
                                        </div>
                                    }
                                }
                            />
                        </div>
                    }.into_any()
                }}
            }.into_any()
        }}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn corridor_geo() -> serde_json::Value {
        json!({
            "type": "FeatureCollection",
            "features": [
                // Map order differs from the list order on purpose.
                {"properties": {"kind": "variant", "variant_id": "b", "color_index": 0},
                 "geometry": {"type": "LineString", "coordinates": [[2.0, 41.0], [2.5, 41.4], [2.2, 40.9]]}},
                {"properties": {"kind": "variant", "variant_id": "a", "color_index": 1},
                 "geometry": {"type": "LineString", "coordinates": [[-3.7, 40.4]]}},
                {"properties": {"kind": "variant", "variant_id": "orphan", "color_index": 2},
                 "geometry": {"type": "LineString", "coordinates": []}},
                {"properties": {"kind": "ors", "preference": "shortest", "color_index": 0},
                 "geometry": {"type": "LineString", "coordinates": [[0.0, 0.0], [1.0, 1.0]]}},
            ]
        })
    }

    #[test]
    fn colors_follow_list_rows_not_map_order() {
        let mut geo = corridor_geo();
        let ids = ["a".to_string(), "b".to_string()];
        let prefs = ["fastest".to_string(), "shortest".to_string()];
        stamp_corridor_colors(&mut geo, &ids, &prefs);
        let p = |i: usize| geo["features"][i]["properties"].clone();
        assert_eq!(p(0)["color_index"], 1);
        assert_eq!(p(0)["route_color"], variant_swatch(1));
        assert_eq!(p(1)["color_index"], 0);
        assert_eq!(p(1)["route_color"], variant_swatch(0));
        // No list row: the map's own color stays.
        assert_eq!(p(2)["color_index"], 2);
        assert!(p(2).get("route_color").is_none());
        assert_eq!(p(3)["color_index"], 1);
        assert_eq!(p(3)["route_color"], ors_swatch(1));
    }

    #[test]
    fn stamping_tolerates_odd_payloads() {
        for mut geo in [
            json!(null),
            json!({}),
            json!({"features": [1, {"properties": null}]}),
        ] {
            let before = geo.clone();
            stamp_corridor_colors(&mut geo, &["a".into()], &[]);
            assert_eq!(geo, before);
        }
    }

    #[test]
    fn palettes_cycle_and_stay_distinct() {
        assert_eq!(variant_swatch(VARIANT_SWATCHES.len()), variant_swatch(0));
        let unique: std::collections::HashSet<_> = VARIANT_SWATCHES.iter().collect();
        assert_eq!(unique.len(), VARIANT_SWATCHES.len());
        assert!(VARIANT_SWATCHES.iter().all(|c| !ORS_SWATCHES.contains(c)));
    }

    #[test]
    fn bounds_cover_the_selected_line_only() {
        let geo = corridor_geo();
        assert_eq!(variant_bounds(&geo, "b"), Some([2.0, 40.9, 2.5, 41.4]));
        // A single point is a degenerate but valid box.
        assert_eq!(variant_bounds(&geo, "a"), Some([-3.7, 40.4, -3.7, 40.4]));
        assert_eq!(variant_bounds(&geo, "orphan"), None);
        assert_eq!(variant_bounds(&geo, "missing"), None);
        // ORS lines are never "variants", whatever their properties say.
        assert_eq!(variant_bounds(&geo, "shortest"), None);
    }

    #[test]
    fn clicking_toggles_selection() {
        assert_eq!(toggle_selection(None, "a"), Some("a".into()));
        assert_eq!(toggle_selection(Some("a"), "a"), None);
        assert_eq!(toggle_selection(Some("a"), "b"), Some("b".into()));
    }
}
