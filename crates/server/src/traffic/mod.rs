//! Trip traffic / congestion estimation from floating-car data + OSM free-flow.

mod frames;
mod job;
mod overpass;
mod score;

pub use job::process_finished_track;
pub use score::{highway_default_kph, level_from_ratio, parse_maxspeed_kph, TrafficLevel};
