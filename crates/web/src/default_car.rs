const DEFAULT_CAR_KEY: &str = "default-car-id";

pub fn load_default_car_id() -> Option<String> {
    let win = web_sys::window()?;
    let storage = win.local_storage().ok()??;
    let val = storage.get_item(DEFAULT_CAR_KEY).ok()??;
    if val.is_empty() { None } else { Some(val) }
}

pub fn save_default_car_id(id: &str) {
    let Some(win) = web_sys::window() else { return };
    let Ok(Some(storage)) = win.local_storage() else { return };
    let _ = storage.set_item(DEFAULT_CAR_KEY, id);
}

pub fn clear_default_car_id() {
    let Some(win) = web_sys::window() else { return };
    let Ok(Some(storage)) = win.local_storage() else { return };
    let _ = storage.remove_item(DEFAULT_CAR_KEY);
}
