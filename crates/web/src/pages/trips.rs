use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;

use crate::api::{get_trip, list_trips, trip_map, trip_points, Trip, TripPoint};
use crate::components::charts::TripCharts;
use crate::components::map::TripMap;
use crate::components::{Icon, IconColor, IconSize};

#[component]
pub fn TripsPage() -> impl IntoView {
    let trips = RwSignal::new(Vec::<Trip>::new());
    let error = RwSignal::new(Option::<String>::None);

    Effect::new(move |_| {
        leptos::task::spawn_local(async move {
            match list_trips(None).await {
                Ok(t) => trips.set(t),
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    });

    view! {
        <div class="topbar">
            <div>
                <h1 class="section-title">
                    <Icon name="map-trifold" color=IconColor::Accent />
                    "Trips"
                </h1>
                <p class="muted">"History across accessible cars"</p>
            </div>
        </div>
        <Show when=move || error.get().is_some()>
            <div class="error">{move || error.get().unwrap_or_default()}</div>
        </Show>
        <div class="card">
            <Show
                when=move || !trips.get().is_empty()
                fallback=move || view! {
                    <div class="empty-state">
                        <Icon name="map-trifold" size=IconSize::Xl color=IconColor::Accent />
                        <div>"No trips yet. Upload a track from the phone to see it here."</div>
                    </div>
                }
            >
                <table class="table">
                    <thead>
                        <tr>
                            <th>"Car"</th><th>"Started"</th><th>"Points"</th>
                            <th>"Distance"</th><th>"Avg speed"</th><th></th>
                        </tr>
                    </thead>
                    <tbody>
                        <For
                            each=move || trips.get()
                            key=|t| t.id.clone()
                            children=move |t| {
                                let id = t.id.clone();
                                view! {
                                    <tr>
                                        <td>{t.car_name.clone()}</td>
                                        <td>{t.started_at.clone()}</td>
                                        <td>{t.point_count}</td>
                                        <td>{format!("{:.1} km", t.distance_m.unwrap_or(0.0)/1000.0)}</td>
                                        <td>{format!("{:.0}", t.avg_speed_kph.unwrap_or(0.0))}</td>
                                        <td>
                                            <A href=format!("/trips/{id}")>
                                                <span class="icon-label">
                                                    "Open"
                                                    <Icon name="caret-right" size=IconSize::Sm />
                                                </span>
                                            </A>
                                        </td>
                                    </tr>
                                }
                            }
                        />
                    </tbody>
                </table>
            </Show>
        </div>
    }
}

#[component]
pub fn TripDetailPage() -> impl IntoView {
    let params = use_params_map();
    let trip = RwSignal::new(Option::<Trip>::None);
    let points = RwSignal::new(Vec::<TripPoint>::new());
    let geojson = RwSignal::new(Option::<serde_json::Value>::None);
    let error = RwSignal::new(Option::<String>::None);

    Effect::new(move |_| {
        let id = params.with(|p| p.get("id").unwrap_or_default());
        if id.is_empty() {
            return;
        }
        leptos::task::spawn_local(async move {
            match get_trip(&id).await {
                Ok(t) => trip.set(Some(t)),
                Err(e) => error.set(Some(e.to_string())),
            }
            match trip_points(&id).await {
                Ok(p) => points.set(p),
                Err(e) => error.set(Some(e.to_string())),
            }
            match trip_map(&id).await {
                Ok(g) => geojson.set(Some(g)),
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    });

    view! {
        <div class="topbar">
            <div>
                <h1>{move || trip.get().map(|t| format!("{} · {}", t.car_name, t.started_at)).unwrap_or_else(|| "Trip".into())}</h1>
                <p class="muted">
                    {move || trip.get().map(|t| format!(
                        "{:.1} km · {:.0} min · max {:.0} km/h · fuel {:.2} L",
                        t.distance_m.unwrap_or(0.0)/1000.0,
                        t.duration_s.unwrap_or(0.0)/60.0,
                        t.max_speed_kph.unwrap_or(0.0),
                        t.fuel_used_l.unwrap_or(0.0)
                    )).unwrap_or_default()}
                </p>
            </div>
            <A href="/trips">
                <span class="icon-label">
                    <Icon name="arrow-left" size=IconSize::Sm />
                    "Back"
                </span>
            </A>
        </div>

        <Show when=move || error.get().is_some()>
            <div class="error">{move || error.get().unwrap_or_default()}</div>
        </Show>

        <div class="grid two">
            <div class="card">
                <h2 class="section-title">
                    <Icon name="map-pin" color=IconColor::Accent />
                    "Route"
                </h2>
                <TripMap geojson=geojson.into()/>
            </div>
            <div class="card">
                <h2 class="section-title">
                    <Icon name="chart-bar" color=IconColor::Accent />
                    "Telemetry"
                </h2>
                <TripCharts points=points.into()/>
            </div>
        </div>
    }
}
