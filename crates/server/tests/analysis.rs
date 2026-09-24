//! Trip analysis: the context handed to the model, and the job lifecycle.
//! Requires DATABASE_URL pointing at Postgres+PostGIS.

#[path = "mcp_support.rs"]
mod support;

use chrono::Utc;
use server::analysis::context::build_trip_analysis_context;
use server::units::UnitSystem;

#[tokio::test]
async fn context_uses_the_shared_trip_stats() {
    let Some(state) = support::state().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let pool = state.pool.clone();
    let user = support::insert_user(&pool).await;
    let car = support::insert_car(&pool, user, "Golf", "GASOLINE", None).await;
    let trip = support::insert_trip(
        &pool,
        car,
        "GASOLINE",
        Utc::now() - chrono::Duration::hours(1),
        &support::cruise(60),
    )
    .await;

    let ctx =
        build_trip_analysis_context(&pool, trip, UnitSystem::Metric, &state.config.overpass_url)
            .await
            .expect("context");

    let o = &ctx.overview;
    assert_eq!(o.fuel_class, "GASOLINE");
    assert_eq!(o.point_count, 60);
    // 59 hops of 0.00012° longitude at 40°N ≈ 0.6 km.
    let d = o.distance_m.expect("distance");
    assert!((500.0..700.0).contains(&d), "distance {d}");
    // Odometer advanced 0.59 km: plausible next to GPS, so economy uses it.
    let econ = o.economy_distance_m.expect("economy distance");
    assert!((580.0..600.0).contains(&econ), "economy {econ}");
    // 3 L/h for 59 s.
    let fuel = o.fuel_used_l.expect("fuel");
    assert!((fuel - 3.0 * 59.0 / 3600.0).abs() < 1e-6, "fuel {fuel}");
    assert_eq!(o.avg_speed_kph, Some(36.0));
    assert_eq!(o.max_speed_kph, Some(36.0));
}

#[tokio::test]
async fn an_electric_trip_has_energy_but_no_liquid_fuel() {
    let Some(state) = support::state().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let pool = state.pool.clone();
    let user = support::insert_user(&pool).await;
    let car = support::insert_car(&pool, user, "Leaf", "FULL_ELECTRIC", Some(40.0)).await;
    let samples: Vec<support::Sample> = support::cruise(30)
        .into_iter()
        .enumerate()
        .map(|(i, mut s)| {
            s.rpm = Some(0.0);
            s.soc_pct = Some(80.0 - i as f64 * 0.1);
            s
        })
        .collect();
    let trip = support::insert_trip(
        &pool,
        car,
        "FULL_ELECTRIC",
        Utc::now() - chrono::Duration::hours(2),
        &samples,
    )
    .await;

    let ctx =
        build_trip_analysis_context(&pool, trip, UnitSystem::Metric, &state.config.overpass_url)
            .await
            .expect("context");

    assert_eq!(ctx.overview.fuel_class, "FULL_ELECTRIC");
    assert_eq!(ctx.overview.fuel_used_l, None);
    assert_eq!(ctx.fuel.fuel_rate_lph_avg, None);
    let kwh = ctx.overview.energy_used_kwh.expect("energy");
    assert!((kwh - 2.9 / 100.0 * 40.0).abs() < 1e-6, "kwh {kwh}");
    // RPM 0 on an EV is not "engine at 0 rpm".
    assert_eq!(ctx.engine.rpm_min, None);
}

#[tokio::test]
async fn a_fixless_trip_still_builds_a_context() {
    let Some(state) = support::state().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let pool = state.pool.clone();
    let user = support::insert_user(&pool).await;
    let car = support::insert_car(&pool, user, "Tunnel", "DIESEL", None).await;
    let samples: Vec<support::Sample> = support::cruise(20)
        .into_iter()
        .enumerate()
        .map(|(i, mut s)| {
            // Only one fix: fewer than two coordinates means no GPS distance.
            if i > 0 {
                s.lat = None;
                s.lon = None;
            }
            s
        })
        .collect();
    let trip = support::insert_trip(
        &pool,
        car,
        "DIESEL",
        Utc::now() - chrono::Duration::hours(3),
        &samples,
    )
    .await;

    let ctx =
        build_trip_analysis_context(&pool, trip, UnitSystem::Metric, &state.config.overpass_url)
            .await
            .expect("context");
    assert_eq!(ctx.overview.point_count, 20);
    assert_eq!(ctx.overview.distance_m, Some(0.0));
    assert!(ctx.overview.fuel_used_l.is_some());
}
