//! Routes Optimization SPA pages.

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::{use_navigate, use_params_map};

use crate::api::{
    list_cars, route_opt_corridor, route_opt_corridor_map, route_opt_recompute, route_opt_summary,
    Car, RouteCorridorDetail, RouteOptSummary,
};
use crate::components::{Icon, IconColor, IconSize};
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

/// Human label for insight `kind` codes from the server.
fn insight_kind_label(kind: &str) -> &'static str {
    match kind {
        "prefer_variant" | "prefer_variant_soft" => "Faster path",
        "avoid_variant_now" | "avoid_variant_now_soft" => "Right now",
        "ors_reference" => "Router tip",
        "ors_matches" => "Matches router",
        "beats_router" => "Beats router",
        "forming" => "Forming",
        "typical_pace" => "Baseline",
        "single_path" => "One path",
        "time_window" => "This hour",
        "peak_vs_offpeak" => "Peak hours",
        "weekend_vs_weekday" => "Weekend",
        "high_stops" => "Stops",
        _ => "Insight",
    }
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
    let navigate = StoredValue::new(use_navigate());

    Effect::new(move |_| {
        leptos::task::spawn_local(async move {
            match list_cars().await {
                Ok(list) => {
                    if car_id.get_untracked().is_empty() {
                        if let Some(c) = list.first() {
                            car_id.set(c.id.clone());
                        }
                    }
                    cars.set(list);
                }
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    });

    Effect::new(move |_| {
        let id = car_id.get();
        if id.is_empty() {
            return;
        }
        error.set(None);
        leptos::task::spawn_local(async move {
            match route_opt_summary(&id).await {
                Ok(s) => summary.set(Some(s)),
                Err(e) => {
                    summary.set(None);
                    error.set(Some(e.to_string()));
                }
            }
        });
    });

    let recompute = move |_| {
        let id = car_id.get();
        if id.is_empty() {
            return;
        }
        busy.set(true);
        message.set(None);
        error.set(None);
        leptos::task::spawn_local(async move {
            match route_opt_recompute(&id).await {
                Ok(r) => {
                    message.set(Some(format!("Recomputed {} trips.", r.processed)));
                    if let Ok(s) = route_opt_summary(&id).await {
                        summary.set(Some(s));
                    }
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            busy.set(false);
        });
    };

    view! {
        <div class="page-header">
            <h1>"Routes"</h1>
            <p class="muted">
                "Compare similar origin→destination corridors, path variants, and OpenRouteService alternatives — no AI."
            </p>
        </div>

        <Show when=move || message.get().is_some()>
            <div class="banner ok">{move || message.get().unwrap_or_default()}</div>
        </Show>
        <Show when=move || error.get().is_some()>
            <div class="banner err">{move || error.get().unwrap_or_default()}</div>
        </Show>

        <div class="card routes-toolbar">
            <div class="form-row" style="margin:0">
                <label>"Car"</label>
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
                {move || if busy.get() { "Recomputing…" } else { "Recompute" }}
            </button>
        </div>

        {move || {
            let s = summary.get();
            let Some(s) = s else {
                return view! { <p class="muted">"Select a car to load corridors."</p> }.into_any();
            };
            if !s.ors_configured {
                view! {
                    <div class="banner warn">
                        "OpenRouteService API key is not set for the car owner. "
                        <A href="/app/settings">"Add it in Settings"</A>
                        " to fetch alternate paths and elevation. Recorded path comparison still works."
                    </div>
                }.into_any()
            } else {
                view! { <></> }.into_any()
            }
        }}

        <h2 class="section-title" style="margin-top:1.25rem">
            <Icon name="star" color=IconColor::Warn />
            "Insights"
        </h2>
        {move || {
            let s = summary.get();
            let insights = s.map(|s| s.insights).unwrap_or_default();
            if insights.is_empty() {
                return view! {
                    <div class="card routes-insight routes-insight-empty">
                        <p class="muted" style="margin:0">
                            "No insights yet. Finish trips on a repeating origin→destination so the corridor can form — baselines appear after the first trip; path comparisons need alternate lines or more samples."
                        </p>
                    </div>
                }.into_any();
            }
            view! {
                <div class="routes-insight-grid">
                    <For
                        each=move || summary.get().map(|s| s.insights).unwrap_or_default()
                        key=|i| i.id.clone()
                        children=move |i| {
                            let kind = i.kind.clone();
                            let kind_label = insight_kind_label(&kind).to_string();
                            let kind_class = format!("routes-insight-kind {}", insight_kind_class(&kind));
                            view! {
                                <div class="card routes-insight">
                                    <div class=kind_class>{kind_label}</div>
                                    <h3>{i.title.clone()}</h3>
                                    <p class="muted" style="margin:0">{i.body.clone()}</p>
                                    <button
                                        class="btn secondary"
                                        style="margin-top:0.65rem"
                                        on:click=move |_| {
                                            let cid = i.corridor_id.clone();
                                            navigate.with_value(|nav| {
                                                nav(&format!("/app/routes/{cid}"), Default::default());
                                            });
                                        }
                                    >
                                        "Open corridor"
                                    </button>
                                </div>
                            }
                        }
                    />
                </div>
            }.into_any()
        }}

        <h2 class="section-title" style="margin-top:1.5rem">
            <Icon name="map-trifold" color=IconColor::Accent />
            "Corridors"
        </h2>
        {move || {
            let s = summary.get();
            let corridors = s.map(|s| s.corridors).unwrap_or_default();
            if corridors.is_empty() {
                return view! {
                    <p class="muted">"No corridors yet. Drive repeating routes and stop tracking so trips can be clustered."</p>
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
                            let dist = c
                                .median_distance
                                .map(|d| fmt_distance(Some(d), &prefs.get()))
                                .unwrap_or_else(|| "—".into());
                            let od_label = if round_trip {
                                if let (Some(vlat), Some(vlon)) = (c.via_lat, c.via_lon) {
                                    format!(
                                        "Round trip via {vlat:.4}, {vlon:.4} · base {:.4}, {:.4}",
                                        c.start_lat, c.start_lon
                                    )
                                } else {
                                    format!(
                                        "Round trip · base {:.4}, {:.4}",
                                        c.start_lat, c.start_lon
                                    )
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
                                            <strong>{format!("{} trips", c.trip_count)}</strong>
                                            <div class="routes-pill-row">
                                                {if round_trip {
                                                    view! { <span class="pill">"Round trip"</span> }.into_any()
                                                } else {
                                                    view! { <span></span> }.into_any()
                                                }}
                                                {if forming {
                                                    view! { <span class="pill warn">"Forming"</span> }.into_any()
                                                } else {
                                                    view! { <span class="pill ok">"Ready"</span> }.into_any()
                                                }}
                                            </div>
                                        </div>
                                        <div class="muted" style="font-size:0.85rem">
                                            {od_label}
                                        </div>
                                        <div class="routes-corridor-metrics">
                                            <span>"Best: "{best}</span>
                                            <span>"Median: "{dur}</span>
                                            <span>"Dist: "{dist}</span>
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

    Effect::new(move |_| {
        let id = params.with(|p| p.get("id").unwrap_or_default());
        if id.is_empty() {
            return;
        }
        leptos::task::spawn_local(async move {
            match route_opt_corridor(&id).await {
                Ok(d) => detail.set(Some(d)),
                Err(e) => error.set(Some(e.to_string())),
            }
            match route_opt_corridor_map(&id).await {
                Ok(g) => map_geo.set(Some(g)),
                Err(_) => map_geo.set(None),
            }
        });
    });

    Effect::new(move |_| {
        let geo = map_geo.get();
        let Some(geo) = geo else { return };
        let Some(el) = map_host.get() else { return };
        crate::components::map::mount_route_opt_map(&el, &geo);
    });

    on_cleanup(move || {
        crate::components::map::dispose_route_opt_map();
    });

    view! {
        <div class="page-header">
            <div>
                <A href="/app/routes"><span class="muted">"← Routes"</span></A>
                <h1>"Corridor"</h1>
            </div>
        </div>
        <Show when=move || error.get().is_some()>
            <div class="banner err">{move || error.get().unwrap_or_default()}</div>
        </Show>

        {move || {
            let d = detail.get();
            let Some(d) = d else {
                return view! { <p class="muted">"Loading…"</p> }.into_any();
            };
            let rec = d.recommendation_for_now.clone();
            let round_trip = d.is_round_trip;
            let via_note = if round_trip {
                if let (Some(vlat), Some(vlon)) = (d.via_lat, d.via_lon) {
                    format!("Round trip corridor via {vlat:.4}, {vlon:.4}")
                } else {
                    "Round trip corridor (start ≈ end)".into()
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
                            {rec.variant_label.clone().unwrap_or_else(|| "No recommendation yet".into())}
                        </strong>
                        <p class="muted" style="margin:0.25rem 0 0">{rec.reason.clone()}</p>
                        <p class="muted" style="margin:0.35rem 0 0;font-size:0.85rem">{via_note}</p>
                    </div>
                    <div class="routes-pill-row">
                        {if round_trip {
                            view! { <span class="pill">"Round trip"</span> }.into_any()
                        } else {
                            view! { <span></span> }.into_any()
                        }}
                        {if d.forming {
                            view! { <span class="pill warn">"Forming · need more trips"</span> }.into_any()
                        } else {
                            view! { <span class="pill ok">{format!("{} trips", d.trip_count)}</span> }.into_any()
                        }}
                    </div>
                </div>

                <div class="card" style="padding:0;overflow:hidden;margin-top:1rem">
                    <div class="routes-opt-map" node_ref=map_host></div>
                    <div class="routes-map-legend">
                        <div class="routes-map-legend-group">
                            <div class="routes-map-legend-title">
                                <span class="routes-line-sample is-variant"></span>
                                "Your path variants"
                                <span class="muted">" · solid"</span>
                            </div>
                            <div class="routes-map-legend-items">
                                {d.variants.iter().enumerate().map(|(i, v)| {
                                    let color = variant_swatch(i).to_string();
                                    let label = v.label.clone();
                                    view! {
                                        <span class="routes-map-swatch-item">
                                            <span class="routes-map-swatch is-variant" style=format!("background:{color}")></span>
                                            {label}
                                        </span>
                                    }
                                }).collect_view()}
                            </div>
                        </div>
                        <div class="routes-map-legend-group">
                            <div class="routes-map-legend-title">
                                <span class="routes-line-sample is-ors"></span>
                                "OpenRouteService alternatives"
                                <span class="muted">" · dashed"</span>
                            </div>
                            <div class="routes-map-legend-items">
                                {
                                    if d.ors_alternatives.is_empty() {
                                        view! { <span class="muted">"None cached yet"</span> }.into_any()
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
                            "Hover a line for the name. Solid = paths you drove · dashed magenta = router estimates."
                        </p>
                    </div>
                </div>

                <h2 class="section-title" style="margin-top:1.25rem">"Path variants"</h2>
                <div class="table-wrap">
                    <table class="table">
                        <thead>
                            <tr>
                                <th>"Variant"</th>
                                <th>"Trips"</th>
                                <th>"Median time"</th>
                                <th>"Median distance"</th>
                                <th>"Stops"</th>
                                <th>"Elev gain"</th>
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
                                                .map(|(i, v)| (i, v))
                                                .collect::<Vec<_>>()
                                        })
                                        .unwrap_or_default()
                                }
                                key=|(_, v)| v.id.clone()
                                children=move |(i, v)| {
                                    let elev = v.median_elev_gain_m.map(|e| format!("{e:.0} m")).unwrap_or_else(|| "—".into());
                                    let dist_label = fmt_distance(Some(v.median_distance), &prefs.get());
                                    let color = variant_swatch(i).to_string();
                                    let label = v.label.clone();
                                    view! {
                                        <tr>
                                            <td>
                                                <span class="routes-name-with-swatch">
                                                    <span class="routes-map-swatch is-variant" style=format!("background:{color}")></span>
                                                    {label}
                                                </span>
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
                    <h2 class="section-title" style="margin-top:1.25rem">"OpenRouteService alternatives"</h2>
                    <div class="table-wrap">
                        <table class="table">
                            <thead>
                                <tr>
                                    <th>"Profile"</th>
                                    <th>"Est. time"</th>
                                    <th>"Distance"</th>
                                    <th>"Ascent"</th>
                                    <th>"Descent"</th>
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
                                                    .map(|(i, a)| (i, a))
                                                    .collect::<Vec<_>>()
                                            })
                                            .unwrap_or_default()
                                    }
                                    key=|(i, a)| format!("{}-{}-{}", i, a.preference, a.fetched_at)
                                    children=move |(i, a)| {
                                        let dist_label = fmt_distance(Some(a.distance), &prefs.get());
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
                                                <td>{a.elev_gain_m.map(|e| format!("{e:.0} m")).unwrap_or_else(|| "—".into())}</td>
                                                <td>{a.elev_loss_m.map(|e| format!("{e:.0} m")).unwrap_or_else(|| "—".into())}</td>
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
                    "Insights"
                </h2>
                {move || {
                    let list = detail.get().map(|d| d.insights).unwrap_or_default();
                    if list.is_empty() {
                        return view! {
                            <div class="card routes-insight routes-insight-empty">
                                <p class="muted" style="margin:0">
                                    "No insights for this corridor yet. They appear automatically from finished trips on this OD — try another path or more drives at different hours for richer tips."
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
