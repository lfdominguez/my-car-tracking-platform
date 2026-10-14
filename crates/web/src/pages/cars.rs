use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;

use crate::api::{
    Car, CreateDeviceResponse, Device, create_car, create_device, get_car, list_cars, list_devices,
    provisioning, provisioning_payload_json, revoke_device, update_car, upload_car_photo,
};
use crate::components::qr::QrCode;
use crate::components::{Icon, IconColor, IconSize};
use crate::i18n::{fuel_class_label, role_label, t};
use crate::pages::alerts::AlertsSection;
use crate::pages::driving::CarScoreChart;
use crate::pages::garage::GarageSection;
use crate::pages::health::HealthSection;
use crate::pages::retention::RetentionCard;
use crate::pages::sharing::SharingCard;
#[component]
pub fn CarsPage() -> impl IntoView {
    let cars = RwSignal::new(Vec::<Car>::new());
    let error = RwSignal::new(Option::<String>::None);
    let name = RwSignal::new(String::new());
    let make_model = RwSignal::new(String::new());
    let default_car = crate::default_car::use_default_car();
    let default_busy = RwSignal::new(false);

    Effect::new(move |_| {
        leptos::task::spawn_local(async move {
            match list_cars().await {
                Ok(c) => {
                    cars.set(c);
                    error.set(None);
                }
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    });

    view! {
        <div class="topbar">
            <div>
                <h1 class="section-title">
                    <Icon name="car" color=IconColor::Accent />
                    {tr!("nav.cars")}
                </h1>
                <p class="muted">{tr!("cars.lead")}</p>
            </div>
        </div>

        <Show when=move || error.get().is_some()>
            <div class="error">{move || error.get().unwrap_or_default()}</div>
        </Show>

        <div class="grid two">
            <div class="card">
                <h2 class="section-title">
                    <Icon name="garage" color=IconColor::Accent />
                    {tr!("dash.your_cars")}
                </h2>
                <Show
                    when=move || !cars.get().is_empty()
                    fallback=move || view! {
                        <div class="empty-state">
                            <Icon name="car" size=IconSize::Xl color=IconColor::Accent />
                            <div>{tr!("cars.no_cars")}</div>
                        </div>
                    }
                >
                    <table class="table">
                        <thead>
                            <tr><th></th><th>{tr!("common.name")}</th><th>{tr!("cars.model")}</th><th>{tr!("common.fuel")}</th><th>{tr!("cars.role")}</th><th></th></tr>
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
                                    let (fuel_class, fuel_type) = (c.fuel_class.clone(), c.fuel_type.clone());
                                    let role = c.role.clone();
                                    let star_id = id.clone();
                                    let is_default = Signal::derive(move || {
                                        default_car.get().as_deref() == Some(star_id.as_str())
                                    });
                                    let toggle_id = id.clone();
                                    let car_name = c.name.clone();
                                    view! {
                                        <tr class:car-row-default=move || is_default.get()>
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
                                                <span class="car-name-cell">
                                                    <button
                                                        type="button"
                                                        class="btn icon-btn car-default-toggle"
                                                        class:is-default=move || is_default.get()
                                                        aria-pressed=move || is_default.get().to_string()
                                                        aria-label=move || crate::i18n::tf("cars.default_toggle_aria", &[("name", &car_name)])
                                                        title=move || if is_default.get() { t("cars.default_clear_hint") } else { t("cars.set_default") }
                                                        disabled=move || default_busy.get()
                                                        on:click=move |_| {
                                                            let next = (!is_default.get_untracked()).then(|| toggle_id.clone());
                                                            default_busy.set(true);
                                                            leptos::task::spawn_local(async move {
                                                                if let Err(e) = crate::default_car::set_default_car(default_car, next).await {
                                                                    let _ = error.try_set(Some(e.to_string()));
                                                                }
                                                                let _ = default_busy.try_set(false);
                                                            });
                                                        }
                                                    >
                                                        <Icon name="star" size=IconSize::Sm />
                                                    </button>
                                                    {c.name.clone()}
                                                    <Show when=move || is_default.get()>
                                                        <span class="badge default-car-badge">{tr!("cars.default_badge")}</span>
                                                    </Show>
                                                </span>
                                            </td>
                                            <td>{c.make_model.clone()}</td>
                                            <td>
                                                <span class="icon-label">
                                                    <Icon name="gas-pump" size=IconSize::Sm color=IconColor::Success />
                                                    {move || format!("{} {}", fuel_class_label(&fuel_class), fuel_type).trim().to_string()}
                                                </span>
                                            </td>
                                            <td>
                                                <span class=format!("badge {}", c.role)>
                                                    <span class="icon-label">
                                                        <Icon name=role_icon size=IconSize::Sm />
                                                        {move || role_label(&role)}
                                                    </span>
                                                </span>
                                            </td>
                                            <td>
                                                <A href=format!("/app/cars/{id}")>
                                                    <span class="icon-label">
                                                        {tr!("cars.manage")}
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
                    {tr!("cars.add_car")}
                </h2>
                <div class="form-row">
                    <label>{tr!("common.name")}</label>
                    <input prop:value=move || name.get() on:input=move |ev| name.set(event_target_value(&ev))/>
                </div>
                <div class="form-row">
                    <label>{tr!("cars.make_model")}</label>
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
                    {tr!("cars.create")}
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
    let error = RwSignal::new(Option::<String>::None);
    let qr_payload = RwSignal::new(Option::<String>::None);
    let last_token = RwSignal::new(Option::<CreateDeviceResponse>::None);

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
    let default_car = crate::default_car::use_default_car();
    let default_busy = RwSignal::new(false);
    let is_default = Signal::derive(move || {
        let id = params.with(|p| p.get("id").unwrap_or_default());
        !id.is_empty() && default_car.get().as_deref() == Some(id.as_str())
    });

    Effect::new({
        move |_| {
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
            });
        }
    });

    view! {
        <div class="topbar">
            <div>
                <h1 class="section-title">
                    <Icon name="car" color=IconColor::Accent />
                    {move || car.get().map(|c| c.name).unwrap_or_else(|| t("common.car").into())}
                </h1>
                <p class="muted">{tr!("cars.detail_lead")}</p>
            </div>
            <A href="/app/cars">
                <span class="icon-label">
                    <Icon name="arrow-left" size=IconSize::Sm />
                    {tr!("common.back")}
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
                    {tr!("cars.profile_fuel")}
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
                                        <img class="car-photo-preview" src=src alt=tr!("cars.photo_alt") />
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
                        <div class="muted" style="font-size:var(--text-sm);margin:0">
                            {tr!("cars.photo_hint")}
                        </div>
                        <div class="car-photo-buttons">
                            <label class="btn secondary car-photo-pick">
                                <Icon name="image" size=IconSize::Sm />
                                {move || if photo_busy.get() { t("cars.uploading") } else { t("cars.change_image") }}
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

                <div class="form-row"><label>{tr!("common.name")}</label>
                    <input prop:value=move || name.get() on:input=move |ev| name.set(event_target_value(&ev))/></div>
                <div class="form-row"><label>{tr!("cars.make_model")}</label>
                    <input prop:value=move || make_model.get() on:input=move |ev| make_model.set(event_target_value(&ev))/></div>
                <div class="form-row"><label>{tr!("cars.powertrain")}</label>
                    <select prop:value=move || fuel_class.get() on:change=move |ev| fuel_class.set(event_target_value(&ev))>
                        <option value="GASOLINE">{tr!("fuel.gasoline")}</option>
                        <option value="DIESEL">{tr!("fuel.diesel")}</option>
                        <option value="HYBRID">{tr!("fuel.hybrid")}</option>
                        <option value="FULL_ELECTRIC">{tr!("fuel.electric")}</option>
                    </select>
                </div>
                <div class="form-row"><label>{tr!("cars.fuel_grade")}</label>
                    <select prop:value=move || fuel_type.get() on:change=move |ev| fuel_type.set(event_target_value(&ev))>
                        <option value="E0">"E0"</option>
                        <option value="E10">"E10"</option>
                        <option value="E27">"E27"</option>
                        <option value="E100">"E100"</option>
                        <option value="B7">{tr!("cars.b7_diesel")}</option>
                        <option value="CUSTOM">"CUSTOM"</option>
                    </select>
                </div>
                <div class="form-row"><label>{tr!("cars.battery_kwh")}</label>
                    <input prop:value=move || battery_kwh.get() on:input=move |ev| battery_kwh.set(event_target_value(&ev)) placeholder=tr!("cars.optional")/>
                </div>
                <div class="form-row"><label>{tr!("cars.stoich")}</label>
                    <input prop:value=move || stoich.get() on:input=move |ev| stoich.set(event_target_value(&ev))/></div>
                <div class="form-row"><label>{tr!("cars.density")}</label>
                    <input prop:value=move || density.get() on:input=move |ev| density.set(event_target_value(&ev))/></div>
                <div class="form-row"><label>{tr!("cars.displacement")}</label>
                    <input prop:value=move || displacement.get() on:input=move |ev| displacement.set(event_target_value(&ev))/></div>
                <div class="form-row"><label>{tr!("cars.ve")}</label>
                    <input prop:value=move || ve.get() on:input=move |ev| ve.set(event_target_value(&ev))/></div>
                <button class="btn primary" on:click=move |_| {
                    let id = params.with(|p| p.get("id").unwrap_or_default());
                    let batt = battery_kwh.get().parse::<f64>().ok().filter(|v| *v > 0.0);
                    let body = serde_json::json!({
                        "name": name.get(),
                        "make_model": make_model.get(),
                        "fuel_type": fuel_type.get(),
                        "fuel_class": fuel_class.get(),
                        "battery_capacity_kwh": batt,
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
                    {tr!("common.save")}
                </button>
                <button
                    class="btn secondary"
                    aria-pressed=move || is_default.get().to_string()
                    disabled=move || default_busy.get()
                    on:click=move |_| {
                        let id = params.with(|p| p.get("id").unwrap_or_default());
                        let next = (!is_default.get_untracked()).then_some(id);
                        default_busy.set(true);
                        leptos::task::spawn_local(async move {
                            if let Err(e) = crate::default_car::set_default_car(default_car, next).await {
                                let _ = error.try_set(Some(e.to_string()));
                            }
                            let _ = default_busy.try_set(false);
                        });
                    }
                >
                    <Icon name="star" />
                    {move || if is_default.get() { t("cars.default_set") } else { t("cars.set_default") }}
                </button>
            </div>

            <div class="card stack">
                <h2 class="section-title">
                    <Icon name="device-mobile" color=IconColor::Device />
                    {tr!("cars.devices_qr")}
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
                    {tr!("cars.create_token")}
                </button>

                <Show when=move || last_token.get().is_some()>
                    <div class="success stack" style="gap:0.5rem">
                        <div>
                            {tr!("cars.token_once")}
                            " "
                            <code>{move || last_token.get().map(|t| t.token).unwrap_or_default()}</code>
                        </div>
                        <p class="muted" style="margin:0;font-size:var(--text-sm)">
                            {tr!("cars.scan_qr")}
                        </p>
                    </div>
                </Show>

                <QrCode payload=qr_payload/>

                <table class="table">
                    <thead><tr><th>{tr!("common.name")}</th><th>{tr!("cars.prefix")}</th><th>{tr!("common.status")}</th><th></th></tr></thead>
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
                                                                if let Some(dev) = list.iter_mut().find(|x| x.id == did)
                                                                    && dev.revoked_at.as_ref().map(|s| s.is_empty()).unwrap_or(true)
                                                                {
                                                                    dev.revoked_at = Some("revoked".into());
                                                                }
                                                            });
                                                            if let Some(tok) = last_token.get_untracked()
                                                                && tok.device.id == did
                                                            {
                                                                last_token.set(None);
                                                                qr_payload.set(None);
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
                                            {tr!("common.revoke")}
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
                                        <td>{move || if is_revoked { t("cars.revoked") } else { t("cars.active") }}</td>
                                        <td>{revoke_btn}</td>
                                    </tr>
                                }
                            }
                        />
                    </tbody>
                </table>
            </div>
        </div>

        <RetentionCard car=car error=error />

        <SharingCard
            car=car
            car_id=Signal::derive(move || params.with(|p| p.get("id").unwrap_or_default()))
            error=error
        />

        <HealthSection
            car_id=Signal::derive(move || params.with(|p| p.get("id").unwrap_or_default()))
            can_edit=Signal::derive(move || {
                car.with(|c| c.as_ref().is_some_and(|c| c.role == "owner" || c.role == "editor"))
            })
            electrified=Signal::derive(move || {
                car.with(|c| {
                    c.as_ref().is_some_and(|c| {
                        c.fuel_class.eq_ignore_ascii_case("FULL_ELECTRIC")
                            || c.fuel_class.eq_ignore_ascii_case("HYBRID")
                    })
                })
            })
        />

        <CarScoreChart car_id=Signal::derive(move || params.with(|p| p.get("id").unwrap_or_default())) />

        <AlertsSection car_id=Signal::derive(move || params.with(|p| p.get("id").unwrap_or_default())) />

        <GarageSection
            car_id=Signal::derive(move || params.with(|p| p.get("id").unwrap_or_default()))
            can_edit=Signal::derive(move || {
                car.with(|c| c.as_ref().is_some_and(|c| c.role == "owner" || c.role == "editor"))
            })
            electric=Signal::derive(move || {
                car.with(|c| c.as_ref().is_some_and(|c| c.fuel_class.eq_ignore_ascii_case("FULL_ELECTRIC")))
            })
        />
    }
}
