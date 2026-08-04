use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;

use crate::api::{
    create_car, create_device, create_share, get_car, list_cars, list_devices, list_shares,
    provisioning, update_car, Car, CreateDeviceResponse, Device, Share,
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
                            <tr><th>"Name"</th><th>"Model"</th><th>"Fuel"</th><th>"Role"</th><th></th></tr>
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
                                    view! {
                                        <tr>
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
                    leptos::task::spawn_local(async move {
                        match create_device(&id, "Android phone").await {
                            Ok(resp) => {
                                let token = resp.token.clone();
                                let device_id = resp.device.id.clone();
                                last_token.set(Some(resp.clone()));
                                match provisioning(&id, &device_id, &token).await {
                                    Ok(payload) => {
                                        if let Ok(text) = serde_json::to_string(&payload) {
                                            qr_payload.set(Some(text));
                                        }
                                    }
                                    Err(e) => error.set(Some(e.to_string())),
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
                    <div class="success">
                        "Token (copy now): "
                        <code>{move || last_token.get().map(|t| t.token).unwrap_or_default()}</code>
                    </div>
                </Show>

                <QrCode payload=qr_payload.into()/>

                <table class="table">
                    <thead><tr><th>"Name"</th><th>"Prefix"</th><th>"Status"</th></tr></thead>
                    <tbody>
                        <For
                            each=move || devices.get()
                            key=|d| d.id.clone()
                            children=move |d| view! {
                                <tr>
                                    <td>
                                        <span class="icon-label">
                                            <Icon name="device-mobile" size=IconSize::Sm color=IconColor::Device />
                                            {d.name.clone()}
                                        </span>
                                    </td>
                                    <td><code>{d.token_prefix.clone()}</code></td>
                                    <td>{if d.revoked_at.is_some() { "revoked" } else { "active" }}</td>
                                </tr>
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
