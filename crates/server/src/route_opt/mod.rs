//! Routes Optimization: corridor clustering, path variants, ORS alternatives, insights.

mod api;
mod geo;
mod insights;
mod job;
mod ors;
mod stats;

pub use api::router;
pub use job::{process_finished_track, recompute_car};
