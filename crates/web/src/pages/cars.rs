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

#[component]
pub fn CarsPage() -> impl IntoView {
    let cars = RwSignal::new(Vec::<Car>::new());
    let error = RwSignal::new(Option::<String>::None);
    let name = RwSignal::new(String::new());
    let make_model = RwSignal::new(String::new());

    let reload = move || {
        leptos::task::spawn_local(async move {
            match list_cars().await {
                Ok(c) => cars.set(c),
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };

    Effect::new(move |_| reload());

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
                                    let thumb = c.photo_path.as_ref().map(|p| format!("/uploads/{p}"));
                                    let has_photo = thumb.is_some();
                                    let thumb_src = thumb.unwrap_or_default();
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
                                            <td>{c.name.clone()}</td>
                                            <td>{c.make_model.clone()}</td>
                                            <td>
                                                <span class="icon-label">
                                                    <Icon name="gas-pump" size=IconSize::Sm color=IconColor::Success />
                                                    {c.fuel_type.clone()}
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
                                                <A href=format!("/cars/{id}")>
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
                    leptos::task::spawn_local(async move {
                        let body = serde_json::json!({ "name": n, "make_model": m });
                        match create_car(&body).await {
                            Ok(_) => {
                                name.set(String::new());
                                make_model.set(String::new());
                                match list_cars().await {
                                    Ok(c) => cars.set(c),
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

    let fuel_type = RwSignal::new("E10".into());
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

    Effect::new(move |_| {
        let id = params.with(|p| p.get("id").unwrap_or_default());
        if id.is_empty() {
            return;
        }
        let id2 = id.clone();
        leptos::task::spawn_local(async move {
            match get_car(&id2).await {
                Ok(c) => {
                    name.set(c.name.clone());
                    make_model.set(c.make_model.clone());
                    fuel_type.set(c.fuel_type.clone());
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
    });

    view! {
        <div class="topbar">
            <div>
                <h1 class="section-title">
                    <Icon name="car" color=IconColor::Accent />
                    {move || car.get().map(|c| c.name).unwrap_or_else(|| "Car".into())}
                </h1>
                <p class="muted">"Fuel settings, devices / QR, and sharing"</p>
            </div>
            <A href="/cars">
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
                            match c.and_then(|c| c.photo_path.clone()) {
                                Some(path) => {
                                    let src = format!("/uploads/{path}?v={rev}");
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
                <div class="form-row"><label>"Fuel type"</label>
                    <select prop:value=move || fuel_type.get() on:change=move |ev| fuel_type.set(event_target_value(&ev))>
                        <option value="E0">"E0"</option>
                        <option value="E10">"E10"</option>
                        <option value="E27">"E27"</option>
                        <option value="E100">"E100"</option>
                        <option value="CUSTOM">"CUSTOM"</option>
                    </select>
                </div>
                <div class="form-row"><label>"Stoich AFR"</label>
                    <input prop:value=move || stoich.get() on:input=move |ev| stoich.set(event_target_value(&ev))/></div>
                <div class="form-row"><label>"Density g/L"</label>
                    <input prop:value=move || density.get() on:input=move |ev| density.set(event_target_value(&ev))/></div>
                <div class="form-row"><label>"Displacement L"</label>
                    <input prop:value=move || displacement.get() on:input=move |ev| displacement.set(event_target_value(&ev))/></div>
                <div class="form-row"><label>"VE"</label>
                    <input prop:value=move || ve.get() on:input=move |ev| ve.set(event_target_value(&ev))/></div>
                <button class="btn primary" on:click=move |_| {
                    let id = params.with(|p| p.get("id").unwrap_or_default());
                    let body = serde_json::json!({
                        "name": name.get(),
                        "make_model": make_model.get(),
                        "fuel_type": fuel_type.get(),
                        "stoich_afr": stoich.get().parse::<f64>().unwrap_or(14.08),
                        "density_gl": density.get().parse::<f64>().unwrap_or(745.0),
                        "displacement_l": displacement.get().parse::<f64>().unwrap_or(1.0),
                        "ve": ve.get().parse::<f64>().unwrap_or(0.85),
                    });
                    leptos::task::spawn_local(async move {
                        match update_car(&id, &body).await {
                            Ok(c) => car.set(Some(c)),
                            Err(e) => error.set(Some(e.to_string())),
                        }
                    });
                }>
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
                <button class="btn" on:click=move |_| {
                    let id = params.with(|p| p.get("id").unwrap_or_default());
                    let email = share_email.get();
                    let role = share_role.get();
                    leptos::task::spawn_local(async move {
                        match create_share(&id, &email, &role).await {
                            Ok(_) => {
                                share_email.set(String::new());
                                match list_shares(&id).await {
                                    Ok(s) => shares.set(s),
                                    Err(e) => error.set(Some(e.to_string())),
                                }
                            }
                            Err(e) => error.set(Some(e.to_string())),
                        }
                    });
                }>
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
                            view! {
                                <tr>
                                    <td>{s.name.clone()}</td>
                                    <td>{s.email.clone()}</td>
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
