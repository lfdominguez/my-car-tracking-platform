use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;

use crate::api::{
    create_car, create_device, create_share, get_car, list_cars, list_devices, list_shares,
    provisioning, provisioning_payload_json, revoke_device, update_car, upload_car_photo, Car,
    CreateDeviceResponse, Device, Share,
};
use crate::components::qr::QrCode;
use crate::components::{Icon, IconColor, IconSize};
use crate::vault::{
    decrypt_car_profile, load_car_dek, put_car_profile, use_vault_session, wrap_and_upload_dek,
    CarProfileV1, VaultUnlockGate,
};

fn placeholder_vault_names(sess: &crate::vault::VaultSession, mut c: Vec<Car>) -> Vec<Car> {
    let unlocked = sess.is_unlocked();
    for car in c.iter_mut() {
        if car.vault_sealed && car.name.is_empty() {
            car.name = if unlocked {
                "🔒 Vault car".into()
            } else {
                "🔒 Locked vault car".into()
            };
        }
    }
    c
}

async fn decrypt_vault_car_names(sess: &crate::vault::VaultSession, mut c: Vec<Car>) -> Vec<Car> {
    if !sess.is_unlocked() {
        return c;
    }
    for car in c.iter_mut() {
        if !car.vault_sealed {
            continue;
        }
        if let Ok(Some(p)) = decrypt_car_profile(sess, &car.id).await {
            car.name = p.name;
            car.make_model = p.make_model;
            car.fuel_type = p.fuel_type;
            car.fuel_class = p.fuel_class;
            car.battery_capacity_kwh = p.battery_capacity_kwh;
            car.notes = p.notes;
        }
    }
    c
}

#[component]
pub fn CarsPage() -> impl IntoView {
    let cars = RwSignal::new(Vec::<Car>::new());
    let error = RwSignal::new(Option::<String>::None);
    let name = RwSignal::new(String::new());
    let make_model = RwSignal::new(String::new());
    // Capture vault session in the reactive owner — not inside spawn_local after await.
    let vault = use_vault_session();

    Effect::new({
        let vault = vault.clone();
        move |_| {
            let sess = vault.clone();
            leptos::task::spawn_local(async move {
                match list_cars().await {
                    Ok(c) => {
                        // Show rows immediately; decrypt names only after the list is visible.
                        let labeled = placeholder_vault_names(&sess, c);
                        cars.set(labeled.clone());
                        error.set(None);
                        if sess.is_unlocked() {
                            let decrypted = decrypt_vault_car_names(&sess, labeled).await;
                            cars.set(decrypted);
                        }
                    }
                    Err(e) => error.set(Some(e.to_string())),
                }
            });
        }
    });

    view! {
        <div class="topbar">
            <div>
                <h1 class="section-title">
                    <Icon name="car" color=IconColor::Accent />
                    "Cars"
                </h1>
                <p class="muted">"Profiles used for fuel math and Android provisioning"</p>
            </div>
        </div>

        <Show when=move || error.get().is_some()>
            <div class="error">{move || error.get().unwrap_or_default()}</div>
        </Show>

        <div class="grid two">
            <div class="card">
                <h2 class="section-title">
                    <Icon name="garage" color=IconColor::Accent />
                    "Your cars"
                </h2>
                <Show
                    when=move || !cars.get().is_empty()
                    fallback=move || view! {
                        <div class="empty-state">
                            <Icon name="car" size=IconSize::Xl color=IconColor::Accent />
                            <div>"No cars yet — add one to start provisioning devices."</div>
                        </div>
                    }
                >
                    <table class="table">
                        <thead>
                            <tr><th></th><th>"Name"</th><th>"Model"</th><th>"Fuel"</th><th>"Role"</th><th></th></tr>
                        </thead>
                        <tbody>
                            <For
                                each=move || cars.get()
                                key=|c| c.id.clone()
                                children=move |c| {
                                    let id = c.id.clone();
                                    let role_icon = match c.role.as_str() {
                                        "owner" => "crown",
                                        "editor" => "pencil-simple",
                                        _ => "eye",
                                    };
                                    let has_photo = c.photo_path.is_some();
                                    let thumb_src = crate::api::car_photo_url(&id, None);
                                    view! {
                                        <tr>
                                            <td class="car-list-thumb-cell">
                                                {if has_photo {
                                                    view! {
                                                        <img class="car-list-thumb" src=thumb_src alt="" />
                                                    }.into_any()
                                                } else {
                                                    view! {
                                                        <div class="car-list-thumb car-list-thumb-fallback">
                                                            <Icon name="car" size=IconSize::Sm color=IconColor::Device />
                                                        </div>
                                                    }.into_any()
                                                }}
                                            </td>
                                            <td>
                                                {c.name.clone()}
                                                {if c.vault_sealed {
                                                    " 🔒".to_string()
                                                } else {
                                                    String::new()
                                                }}
                                            </td>
                                            <td>{c.make_model.clone()}</td>
                                            <td>
                                                <span class="icon-label">
                                                    <Icon name="gas-pump" size=IconSize::Sm color=IconColor::Success />
                                                    {format!("{} {}", c.fuel_class, c.fuel_type).trim().to_string()}
                                                </span>
                                            </td>
                                            <td>
                                                <span class=format!("badge {}", c.role)>
                                                    <span class="icon-label">
                                                        <Icon name=role_icon size=IconSize::Sm />
                                                        {c.role.clone()}
                                                    </span>
                                                </span>
                                            </td>
                                            <td>
                                                <A href=format!("/app/cars/{id}")>
                                                    <span class="icon-label">
                                                        "Manage"
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

            <div class="card">
                <h2 class="section-title">
                    <Icon name="plus-circle" color=IconColor::Accent />
                    "Add car"
                </h2>
                <div class="form-row">
                    <label>"Name"</label>
                    <input prop:value=move || name.get() on:input=move |ev| name.set(event_target_value(&ev))/>
                </div>
                <div class="form-row">
                    <label>"Make / model"</label>
                    <input prop:value=move || make_model.get() on:input=move |ev| make_model.set(event_target_value(&ev))/>
                </div>
                <button class="btn primary" on:click=move |_| {
                    let n = name.get();
                    let m = make_model.get();
                    let sess = vault.clone();
                    leptos::task::spawn_local(async move {
                        let body = serde_json::json!({ "name": n, "make_model": m });
                        match create_car(&body).await {
                            Ok(created) => {
                                if created.vault_sealed {
                                    if sess.is_unlocked() {
                                        let profile = CarProfileV1 {
                                            name: n.clone(),
                                            make_model: m.clone(),
                                            fuel_type: created.fuel_type.clone(),
                                            fuel_class: created.fuel_class.clone(),
                                            battery_capacity_kwh: created.battery_capacity_kwh,
                                            stoich_afr: created.stoich_afr,
                                            density_gl: created.density_gl,
                                            displacement_l: created.displacement_l,
                                            ve: created.ve,
                                            notes: created.notes.clone(),
                                        };
                                        if let Err(e) = put_car_profile(&sess, &created.id, &profile).await {
                                            error.set(Some(format!("Car created but vault seal failed: {e}")));
                                        }
                                    } else {
                                        error.set(Some(
                                            "Car created under vault — unlock and edit to seal profile.".into(),
                                        ));
                                    }
                                }
                                name.set(String::new());
                                make_model.set(String::new());
                                match list_cars().await {
                                    Ok(c) => {
                                        let labeled = placeholder_vault_names(&sess, c);
                                        cars.set(labeled.clone());
                                        if sess.is_unlocked() {
                                            let decrypted =
                                                decrypt_vault_car_names(&sess, labeled).await;
                                            cars.set(decrypted);
                                        }
                                    }
                                    Err(e) => error.set(Some(e.to_string())),
                                }
                            }
                            Err(e) => error.set(Some(e.to_string())),
                        }
                    });
                }>
                    <Icon name="plus-circle" />
                    "Create"
                </button>
            </div>
        </div>
    }
}

#[component]
pub fn CarDetailPage() -> impl IntoView {
    let params = use_params_map();
    let car = RwSignal::new(Option::<Car>::None);
    let devices = RwSignal::new(Vec::<Device>::new());
    let shares = RwSignal::new(Vec::<Share>::new());
    let error = RwSignal::new(Option::<String>::None);
    let qr_payload = RwSignal::new(Option::<String>::None);
    let last_token = RwSignal::new(Option::<CreateDeviceResponse>::None);
    let share_email = RwSignal::new(String::new());
    let share_role = RwSignal::new("viewer".to_string());

    let fuel_class = RwSignal::new("GASOLINE".into());
    let fuel_type = RwSignal::new("E10".into());
    let battery_kwh = RwSignal::new(String::new());
    let stoich = RwSignal::new("14.08".into());
    let density = RwSignal::new("745".into());
    let displacement = RwSignal::new("1.0".into());
    let ve = RwSignal::new("0.85".into());
    let name = RwSignal::new(String::new());
    let make_model = RwSignal::new(String::new());
    let photo_busy = RwSignal::new(false);
    let photo_input: NodeRef<leptos::html::Input> = NodeRef::new();
    // Cache-buster so the browser reloads the image after upload.
    let photo_rev = RwSignal::new(0u32);

    let vault = use_vault_session();

    Effect::new({
        let vault = vault.clone();
        move |_| {
        let id = params.with(|p| p.get("id").unwrap_or_default());
        if id.is_empty() {
            return;
        }
        let id2 = id.clone();
        let sess = vault.clone();
        leptos::task::spawn_local(async move {
            match get_car(&id2).await {
                Ok(mut c) => {
                    if c.vault_sealed && sess.is_unlocked() {
                        match decrypt_car_profile(&sess, &id2).await {
                            Ok(Some(p)) => {
                                c.name = p.name.clone();
                                c.make_model = p.make_model.clone();
                                c.fuel_type = p.fuel_type.clone();
                                c.fuel_class = p.fuel_class.clone();
                                c.battery_capacity_kwh = p.battery_capacity_kwh;
                                c.stoich_afr = p.stoich_afr;
                                c.density_gl = p.density_gl;
                                c.displacement_l = p.displacement_l;
                                c.ve = p.ve;
                                c.notes = p.notes.clone();
                            }
                            Ok(None) => {
                                error.set(Some(
                                    "Vault car has no sealed profile yet.".into(),
                                ));
                            }
                            Err(e) => error.set(Some(e)),
                        }
                    }
                    name.set(c.name.clone());
                    make_model.set(c.make_model.clone());
                    fuel_type.set(c.fuel_type.clone());
                    fuel_class.set(if c.fuel_class.is_empty() {
                        "GASOLINE".into()
                    } else {
                        c.fuel_class.clone()
                    });
                    battery_kwh.set(
                        c.battery_capacity_kwh
                            .map(|v| v.to_string())
                            .unwrap_or_default(),
                    );
                    stoich.set(c.stoich_afr.to_string());
                    density.set(c.density_gl.to_string());
                    displacement.set(c.displacement_l.to_string());
                    ve.set(c.ve.to_string());
                    car.set(Some(c));
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            match list_devices(&id2).await {
                Ok(d) => devices.set(d),
                Err(e) => error.set(Some(e.to_string())),
            }
            match list_shares(&id2).await {
                Ok(s) => shares.set(s),
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    }});

    view! {
        <div class="topbar">
            <div>
                <h1 class="section-title">
                    <Icon name="car" color=IconColor::Accent />
                    {move || car.get().map(|c| c.name).unwrap_or_else(|| "Car".into())}
                </h1>
                <p class="muted">"Fuel settings, devices / QR, and sharing"</p>
            </div>
            <A href="/app/cars">
                <span class="icon-label">
                    <Icon name="arrow-left" size=IconSize::Sm />
                    "Back"
                </span>
            </A>
        </div>

        <Show when=move || error.get().is_some()>
            <div class="error">{move || error.get().unwrap_or_default()}</div>
        </Show>
        <Show when=move || car.get().map(|c| c.vault_sealed).unwrap_or(false) && !use_vault_session().is_unlocked()>
            <VaultUnlockGate message="Unlock the vault to view or edit this sealed car profile.".to_string()/>
        </Show>


        <div class="grid two">
            <div class="card stack">
                <h2 class="section-title">
                    <Icon name="engine" color=IconColor::Warn />
                    "Profile & fuel"
                </h2>

                <div class="car-photo-editor">
                    <div class="car-photo-preview-wrap">
                        {move || {
                            let c = car.get();
                            let rev = photo_rev.get();
                            match c.as_ref().and_then(|c| c.photo_path.as_ref().map(|_| c.id.clone())) {
                                Some(car_id) => {
                                    let src = crate::api::car_photo_url(&car_id, Some(rev));
                                    view! {
                                        <img class="car-photo-preview" src=src alt="Car photo" />
                                    }
                                    .into_any()
                                }
                                None => view! {
                                    <div class="car-photo-preview car-photo-preview-fallback">
                                        <Icon name="car" size=IconSize::Xl color=IconColor::Device />
                                    </div>
                                }
                                .into_any(),
                            }
                        }}
                    </div>
                    <div class="car-photo-actions stack" style="gap:0.45rem;flex:1;min-width:0">
                        <div class="muted" style="font-size:0.85rem;margin:0">
                            "Car image shown on the dashboard. JPEG, PNG, WebP, or GIF · max 8 MB."
                        </div>
                        <div class="car-photo-buttons">
                            <label class="btn secondary car-photo-pick">
                                <Icon name="image" size=IconSize::Sm />
                                {move || if photo_busy.get() { "Uploading…" } else { "Change image" }}
                                <input
                                    type="file"
                                    accept="image/jpeg,image/png,image/webp,image/gif,.jpg,.jpeg,.png,.webp,.gif"
                                    node_ref=photo_input
                                    disabled=move || photo_busy.get()
                                    style="display:none"
                                    on:change=move |_| {
                                        let Some(input) = photo_input.get() else { return };
                                        let Some(files) = input.files() else { return };
                                        let Some(file) = files.get(0) else { return };
                                        let id = params.with(|p| p.get("id").unwrap_or_default());
                                        if id.is_empty() {
                                            return;
                                        }
                                        error.set(None);
                                        photo_busy.set(true);
                                        leptos::task::spawn_local(async move {
                                            match upload_car_photo(&id, &file).await {
                                                Ok(c) => {
                                                    car.set(Some(c));
                                                    photo_rev.update(|n| *n = n.wrapping_add(1));
                                                    if let Some(el) = photo_input.get_untracked() {
                                                        el.set_value("");
                                                    }
                                                }
                                                Err(e) => error.set(Some(e.to_string())),
                                            }
                                            photo_busy.set(false);
                                        });
                                    }
                                />
                            </label>
                        </div>
                    </div>
                </div>

                <div class="form-row"><label>"Name"</label>
                    <input prop:value=move || name.get() on:input=move |ev| name.set(event_target_value(&ev))/></div>
                <div class="form-row"><label>"Make / model"</label>
                    <input prop:value=move || make_model.get() on:input=move |ev| make_model.set(event_target_value(&ev))/></div>
                <div class="form-row"><label>"Powertrain"</label>
                    <select prop:value=move || fuel_class.get() on:change=move |ev| fuel_class.set(event_target_value(&ev))>
                        <option value="GASOLINE">"Gasoline"</option>
                        <option value="DIESEL">"Diesel"</option>
                        <option value="HYBRID">"Hybrid"</option>
                        <option value="FULL_ELECTRIC">"Full Electric"</option>
                    </select>
                </div>
                <div class="form-row"><label>"Fuel grade"</label>
                    <select prop:value=move || fuel_type.get() on:change=move |ev| fuel_type.set(event_target_value(&ev))>
                        <option value="E0">"E0"</option>
                        <option value="E10">"E10"</option>
                        <option value="E27">"E27"</option>
                        <option value="E100">"E100"</option>
                        <option value="B7">"B7 diesel"</option>
                        <option value="CUSTOM">"CUSTOM"</option>
                    </select>
                </div>
                <div class="form-row"><label>"HV battery kWh"</label>
                    <input prop:value=move || battery_kwh.get() on:input=move |ev| battery_kwh.set(event_target_value(&ev)) placeholder="optional"/>
                </div>
                <div class="form-row"><label>"Stoich AFR"</label>
                    <input prop:value=move || stoich.get() on:input=move |ev| stoich.set(event_target_value(&ev))/></div>
                <div class="form-row"><label>"Density g/L"</label>
                    <input prop:value=move || density.get() on:input=move |ev| density.set(event_target_value(&ev))/></div>
                <div class="form-row"><label>"Displacement L"</label>
                    <input prop:value=move || displacement.get() on:input=move |ev| displacement.set(event_target_value(&ev))/></div>
                <div class="form-row"><label>"VE"</label>
                    <input prop:value=move || ve.get() on:input=move |ev| ve.set(event_target_value(&ev))/></div>
                <button class="btn primary" on:click={
                    let vault = vault.clone();
                    move |_| {
                    let id = params.with(|p| p.get("id").unwrap_or_default());
                    let sealed = car.get().map(|c| c.vault_sealed).unwrap_or(false);
                    let batt = battery_kwh.get().parse::<f64>().ok().filter(|v| *v > 0.0);
                    let profile = CarProfileV1 {
                        name: name.get(),
                        make_model: make_model.get(),
                        fuel_type: fuel_type.get(),
                        fuel_class: fuel_class.get(),
                        battery_capacity_kwh: batt,
                        stoich_afr: stoich.get().parse::<f64>().unwrap_or(14.08),
                        density_gl: density.get().parse::<f64>().unwrap_or(745.0),
                        displacement_l: displacement.get().parse::<f64>().unwrap_or(1.0),
                        ve: ve.get().parse::<f64>().unwrap_or(0.85),
                        notes: None,
                    };
                    let body = serde_json::json!({
                        "name": profile.name,
                        "make_model": profile.make_model,
                        "fuel_type": profile.fuel_type,
                        "fuel_class": profile.fuel_class,
                        "battery_capacity_kwh": profile.battery_capacity_kwh,
                        "stoich_afr": profile.stoich_afr,
                        "density_gl": profile.density_gl,
                        "displacement_l": profile.displacement_l,
                        "ve": profile.ve,
                    });
                    let sess = vault.clone();
                    leptos::task::spawn_local(async move {
                        if sealed {
                            if !sess.is_unlocked() {
                                error.set(Some("Unlock vault to save sealed car profile.".into()));
                                return;
                            }
                            if let Err(e) = put_car_profile(&sess, &id, &profile).await {
                                error.set(Some(e));
                                return;
                            }
                            // Keep server skeleton non-sensitive.
                            let blank = serde_json::json!({
                                "name": "",
                                "make_model": "",
                                "fuel_type": profile.fuel_type,
                                "fuel_class": profile.fuel_class,
                                "battery_capacity_kwh": profile.battery_capacity_kwh,
                                "stoich_afr": profile.stoich_afr,
                                "density_gl": profile.density_gl,
                                "displacement_l": profile.displacement_l,
                                "ve": profile.ve,
                            });
                            match update_car(&id, &blank).await {
                                Ok(mut c) => {
                                    c.name = profile.name;
                                    c.make_model = profile.make_model;
                                    c.fuel_type = profile.fuel_type;
                                    c.fuel_class = profile.fuel_class;
                                    c.battery_capacity_kwh = profile.battery_capacity_kwh;
                                    c.vault_sealed = true;
                                    car.set(Some(c));
                                }
                                Err(e) => error.set(Some(e.to_string())),
                            }
                        } else {
                            match update_car(&id, &body).await {
                                Ok(c) => car.set(Some(c)),
                                Err(e) => error.set(Some(e.to_string())),
                            }
                        }
                    });
                }}>
                    <Icon name="floppy-disk" />
                    "Save"
                </button>
            </div>

            <div class="card stack">
                <h2 class="section-title">
                    <Icon name="device-mobile" color=IconColor::Device />
                    "Devices & QR"
                </h2>
                <button class="btn primary" on:click=move |_| {
                    let id = params.with(|p| p.get("id").unwrap_or_default());
                    let car_snapshot = car.get();
                    error.set(None);
                    leptos::task::spawn_local(async move {
                        match create_device(&id, "Android phone").await {
                            Ok(resp) => {
                                let token = resp.token.clone();
                                let device_id = resp.device.id.clone();
                                last_token.set(Some(resp.clone()));

                                // Prefer client-side QR JSON (token is only available once).
                                let qr_text = if let Some(ref c) = car_snapshot {
                                    provisioning_payload_json(&token, c)
                                } else {
                                    Err(crate::api::ApiError::Message("car not loaded".into()))
                                };

                                match qr_text {
                                    Ok(text) => qr_payload.set(Some(text)),
                                    Err(_) => {
                                        // Fallback: server rebuilds URLs + fuel fields with ?token=
                                        match provisioning(&id, &device_id, &token).await {
                                            Ok(payload) => {
                                                match serde_json::to_string(&payload) {
                                                    Ok(text) => qr_payload.set(Some(text)),
                                                    Err(e) => error.set(Some(e.to_string())),
                                                }
                                            }
                                            Err(e) => error.set(Some(e.to_string())),
                                        }
                                    }
                                }

                                match list_devices(&id).await {
                                    Ok(d) => devices.set(d),
                                    Err(e) => error.set(Some(e.to_string())),
                                }
                            }
                            Err(e) => error.set(Some(e.to_string())),
                        }
                    });
                }>
                    <Icon name="qr-code" color=IconColor::Default />
                    "Create device token"
                </button>

                <Show when=move || last_token.get().is_some()>
                    <div class="success stack" style="gap:0.5rem">
                        <div>
                            "Token (copy now — shown once): "
                            <code>{move || last_token.get().map(|t| t.token).unwrap_or_default()}</code>
                        </div>
                        <p class="muted" style="margin:0;font-size:0.85rem">
                            "Scan the QR below in the Android app Settings to load URLs, token, and fuel profile."
                        </p>
                    </div>
                </Show>

                <QrCode payload=qr_payload/>

                <table class="table">
                    <thead><tr><th>"Name"</th><th>"Prefix"</th><th>"Status"</th><th></th></tr></thead>
                    <tbody>
                        <For
                            each=move || devices.get()
                            // Include revoked_at so status changes remount the row.
                            // For only reuses children when the key is unchanged.
                            key=|d| {
                                format!(
                                    "{}:{}",
                                    d.id,
                                    d.revoked_at.as_deref().unwrap_or("")
                                )
                            }
                            children=move |d| {
                                let device_id = d.id.clone();
                                // Prefer the route car id (always present on this page) over the
                                // row payload, so revoke cannot 404 from a missing/empty car_id.
                                let car_id_row = {
                                    let from_route = params.with(|p| p.get("id").unwrap_or_default());
                                    if from_route.is_empty() {
                                        d.car_id.clone()
                                    } else {
                                        from_route
                                    }
                                };
                                let is_revoked = d
                                    .revoked_at
                                    .as_ref()
                                    .map(|s| !s.is_empty())
                                    .unwrap_or(false);
                                let revoke_btn = (!is_revoked).then(|| {
                                    let did = device_id.clone();
                                    let cid = car_id_row.clone();
                                    view! {
                                        <button
                                            class="btn"
                                            type="button"
                                            on:click=move |_| {
                                                let did = did.clone();
                                                let cid = cid.clone();
                                                error.set(None);
                                                leptos::task::spawn_local(async move {
                                                    match revoke_device(&cid, &did).await {
                                                        Ok(()) => {
                                                            // Optimistic UI: mark revoked immediately so
                                                            // the row updates before the list refetch.
                                                            devices.update(|list| {
                                                                if let Some(dev) = list.iter_mut().find(|x| x.id == did) {
                                                                    if dev.revoked_at.as_ref().map(|s| s.is_empty()).unwrap_or(true) {
                                                                        dev.revoked_at = Some("revoked".into());
                                                                    }
                                                                }
                                                            });
                                                            if let Some(tok) = last_token.get_untracked() {
                                                                if tok.device.id == did {
                                                                    last_token.set(None);
                                                                    qr_payload.set(None);
                                                                }
                                                            }
                                                            match list_devices(&cid).await {
                                                                Ok(list) => devices.set(list),
                                                                Err(e) => error.set(Some(e.to_string())),
                                                            }
                                                        }
                                                        Err(e) => error.set(Some(e.to_string())),
                                                    }
                                                });
                                            }
                                        >
                                            <Icon name="trash" size=IconSize::Sm color=IconColor::Danger />
                                            "Revoke"
                                        </button>
                                    }
                                });
                                view! {
                                    <tr>
                                        <td>
                                            <span class="icon-label">
                                                <Icon name="device-mobile" size=IconSize::Sm color=IconColor::Device />
                                                {d.name.clone()}
                                            </span>
                                        </td>
                                        <td><code>{d.token_prefix.clone()}</code></td>
                                        <td>{if is_revoked { "revoked" } else { "active" }}</td>
                                        <td>{revoke_btn}</td>
                                    </tr>
                                }
                            }
                        />
                    </tbody>
                </table>
            </div>
        </div>

        <div class="card" style="margin-top:1rem">
            <h2 class="section-title">
                <Icon name="share-network" color=IconColor::Accent />
                "Sharing"
            </h2>
            <div class="row">
                <input style="max-width:260px" placeholder="user@email.com"
                    prop:value=move || share_email.get()
                    on:input=move |ev| share_email.set(event_target_value(&ev))/>
                <select style="max-width:140px" prop:value=move || share_role.get()
                    on:change=move |ev| share_role.set(event_target_value(&ev))>
                    <option value="viewer">"viewer"</option>
                    <option value="editor">"editor"</option>
                </select>
                <button class="btn" on:click={
                    let vault = vault.clone();
                    move |_| {
                    let id = params.with(|p| p.get("id").unwrap_or_default());
                    let email = share_email.get();
                    let role = share_role.get();
                    let sealed = car.get().map(|c| c.vault_sealed).unwrap_or(false);
                    let sess = vault.clone();
                    leptos::task::spawn_local(async move {
                        match create_share(&id, &email, &role).await {
                            Ok(resp) => {
                                if sealed {
                                    if let Some(share) = resp.share.as_ref() {
                                        if let Some(pk) = share.vault_identity_pubkey_b64.as_ref() {
                                            if sess.is_unlocked() {
                                                match load_car_dek(&sess, &id).await {
                                                    Ok(dek) => {
                                                        if let Err(e) = wrap_and_upload_dek(
                                                            &sess,
                                                            &id,
                                                            &share.user_id,
                                                            pk,
                                                            &dek,
                                                        )
                                                        .await
                                                        {
                                                            error.set(Some(format!(
                                                                "Share added but DEK wrap failed: {e}"
                                                            )));
                                                        }
                                                    }
                                                    Err(e) => error.set(Some(format!(
                                                        "Share added but could not load DEK: {e}"
                                                    ))),
                                                }
                                            } else {
                                                error.set(Some(
                                                    "Share added — unlock vault to wrap the car key for the recipient.".into(),
                                                ));
                                            }
                                        } else {
                                            error.set(Some(
                                                "Share added — recipient has no vault pubkey yet (pending wrap).".into(),
                                            ));
                                        }
                                    }
                                }
                                share_email.set(String::new());
                                match list_shares(&id).await {
                                    Ok(s) => shares.set(s),
                                    Err(e) => error.set(Some(e.to_string())),
                                }
                            }
                            Err(e) => error.set(Some(e.to_string())),
                        }
                    });
                }}>
                    <Icon name="user-plus" />
                    "Invite"
                </button>
            </div>
            <table class="table">
                <thead><tr><th>"User"</th><th>"Email"</th><th>"Role"</th></tr></thead>
                <tbody>
                    <For
                        each=move || shares.get()
                        key=|s| format!("{}:{}", s.car_id, s.user_id)
                        children=move |s| {
                            let role_icon = match s.role.as_str() {
                                "editor" => "pencil-simple",
                                _ => "eye",
                            };
                            let vault_hint = if s.vault_has_pubkey {
                                ""
                            } else {
                                " · no vault key"
                            };
                            view! {
                                <tr>
                                    <td>{s.name.clone()}</td>
                                    <td>{s.email.clone()}{vault_hint}</td>
                                    <td>
                                        <span class=format!("badge {}", s.role)>
                                            <span class="icon-label">
                                                <Icon name=role_icon size=IconSize::Sm />
                                                {s.role.clone()}
                                            </span>
                                        </span>
                                    </td>
                                </tr>
                            }
                        }
                    />
                </tbody>
            </table>
        </div>
    }
}
