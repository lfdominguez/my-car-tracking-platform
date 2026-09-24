//! The user's default car: preselected in every car filter and on the live map,
//! so a car someone shared with you doesn't crowd your own views.
//!
//! It lives on the account (`default_car_id` on `/api/me`) so it follows the user
//! across devices. localStorage keeps a copy only so a page can read it the moment
//! it mounts, before `/api/me` has answered; the shell overwrites that copy with
//! the server's value on every load.

use leptos::prelude::*;

const DEFAULT_CAR_KEY: &str = "default-car-id";
/// Set once a device's pre-account default has been offered to the server, so a
/// default cleared elsewhere isn't brought back by this device's stale copy.
const MIGRATED_KEY: &str = "default-car-migrated";

/// The default car id, or `None` for "all cars".
pub type DefaultCarSignal = RwSignal<Option<String>>;

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

fn load_cached() -> Option<String> {
    let val = storage()?.get_item(DEFAULT_CAR_KEY).ok()??;
    if val.is_empty() { None } else { Some(val) }
}

fn cache(id: Option<&str>) {
    let Some(s) = storage() else { return };
    let _ = match id {
        Some(id) => s.set_item(DEFAULT_CAR_KEY, id),
        None => s.remove_item(DEFAULT_CAR_KEY),
    };
}

/// Called once by the app shell.
pub fn provide_default_car() -> DefaultCarSignal {
    let sig = RwSignal::new(load_cached());
    provide_context(sig);
    sig
}

pub fn use_default_car() -> DefaultCarSignal {
    use_context::<DefaultCarSignal>().unwrap_or_else(|| RwSignal::new(load_cached()))
}

/// Adopt the server's value after `/api/me` loads.
///
/// A device that picked a default before it was stored on the account pushes that
/// one up the first time, instead of losing it.
pub fn sync_from_server(sig: DefaultCarSignal, server: Option<String>) {
    let migrated = storage()
        .and_then(|s| s.get_item(MIGRATED_KEY).ok().flatten())
        .is_some();
    if let Some(s) = storage() {
        let _ = s.set_item(MIGRATED_KEY, "1");
    }
    match (server, load_cached()) {
        (None, Some(local)) if !migrated => {
            leptos::task::spawn_local(async move {
                let _ = set_default_car(sig, Some(local)).await;
            });
        }
        (server, _) => {
            cache(server.as_deref());
            sig.set(server);
        }
    }
}

/// Store `id` (or clear it with `None`) on the account, then everywhere else.
pub async fn set_default_car(
    sig: DefaultCarSignal,
    id: Option<String>,
) -> Result<(), crate::api::ApiError> {
    let body = serde_json::json!({ "default_car_id": id.clone().unwrap_or_default() });
    match crate::api::update_me_preferences(body).await {
        Ok(me) => {
            cache(me.default_car_id.as_deref());
            let _ = sig.try_set(me.default_car_id);
            Ok(())
        }
        Err(e) => {
            // The car is gone or no longer shared: stop offering it.
            if id.is_some() && load_cached() == id {
                cache(None);
                let _ = sig.try_set(None);
            }
            Err(e)
        }
    }
}

/// The car filter of a page with an "All cars" option: `""` means all.
///
/// Starts at `preset` (e.g. a `?car_id=` from the URL) or else the default car.
/// Without a preset it also picks up a default that only arrives after mount
/// (first visit on a new device), as long as the user hasn't changed it yet.
pub fn car_filter(preset: Option<String>) -> RwSignal<String> {
    let default = use_default_car();
    let has_preset = preset.is_some();
    let initial = preset
        .or_else(|| default.get_untracked())
        .unwrap_or_default();
    let filter = RwSignal::new(initial.clone());
    if !has_preset {
        let applied = StoredValue::new(initial);
        Effect::new(move |_| {
            let next = default.get().unwrap_or_default();
            if filter.get_untracked() == applied.get_value() && next != applied.get_value() {
                filter.set(next.clone());
            }
            applied.set_value(next);
        });
    }
    filter
}
