//! Routes Optimization: corridor clustering, path variants, ORS alternatives, insights.

mod api;
mod geo;
mod insights;
mod job;
mod ors;
mod stats;

pub use api::router;
pub use geo::{haversine_m, LatLon};
pub use job::{
    clear_track_route_assignments, process_finished_track, prune_empty_corridors_for_car,
    recompute_car, sync_corridor_trip_counts, sync_corridors,
};
