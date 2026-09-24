//! Garage: maintenance schedule, service log, odometer readings and the fuel &
//! charging log of one car (#113, #114, #115).
//!
//! The endpoints speak SI (km, litres, price per litre) whatever the user's unit
//! system; everything shown or typed here goes through the `units` converters.
//! Owners and editors get the forms; viewers see the same data read-only.

use leptos::prelude::*;

use crate::api::{
    DueItem, DueResponse, FuelEntry, FuelSummary, MaintenanceItem, MaintenanceLogEntry,
    add_odometer, create_fuel_entry, create_maintenance_item, create_maintenance_log,
    delete_fuel_entry, delete_maintenance_item, delete_maintenance_log, fuel_summary,
    list_fuel_log, list_maintenance_items, list_maintenance_log, maintenance_due,
    update_maintenance_item,
};
use crate::components::{Icon, IconColor, IconSize};
use crate::i18n::{num, t, tf};
use crate::units::{
    UnitPrefs, UnitSystem, display_to_km, display_to_litres, display_to_price_per_litre, fmt_km,
    fmt_money, km_to_display, l_per_100km_to_display, litres_to_display,
    price_per_litre_to_display, use_unit_prefs,
};

/// Parse a user-typed number, accepting a decimal comma.
fn parse_num(raw: &str) -> Option<f64> {
    let t = raw.trim().replace(',', ".");
    if t.is_empty() {
        return None;
    }
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Trimmed text, or `None` when blank.
fn opt_text(raw: &str) -> Option<String> {
    Some(raw.trim().to_string()).filter(|s| !s.is_empty())
}

fn confirm(msg: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.confirm_with_message(msg).ok())
        .unwrap_or(false)
}

/// Local date and time for an RFC3339 instant, in the locale's date order.
fn local_time(s: &str) -> String {
    crate::i18n::local_datetime(s, None)
}

/// A `datetime-local` value (browser local time) as RFC3339 UTC.
fn local_input_to_rfc3339(raw: &str) -> Option<String> {
    use chrono::{Local, NaiveDateTime, TimeZone};
    let naive = NaiveDateTime::parse_from_str(raw.trim(), "%Y-%m-%dT%H:%M").ok()?;
    let local = Local.from_local_datetime(&naive).earliest()?;
    Some(
        local
            .with_timezone(&chrono::Utc)
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    )
}

fn today_input() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// `(pill class, label)` for a due status.
fn due_badge(status: &str) -> (&'static str, &'static str) {
    match status {
        "overdue" => ("pill pill-danger", t("garage.overdue")),
        "soon" => ("pill pill-warn", t("garage.due_soon")),
        "ok" => ("pill pill-ok", t("garage.ok")),
        _ => ("pill", t("garage.no_baseline")),
    }
}

/// "in 12 days · 800 km left" for one due item.
fn due_detail(item: &DueItem, prefs: &UnitPrefs) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(d) = item.days_left {
        let when = crate::i18n::iso_date(&item.due_on.clone().unwrap_or_default());
        parts.push(match d {
            d if d < 0 => tf("garage.days_late", &[("when", &when), ("n", &-d)]),
            0 => tf("garage.today", &[("when", &when)]),
            d => tf("garage.in_days", &[("when", &when), ("n", &d)]),
        });
    }
    if let Some(k) = item.km_left {
        let v = num(km_to_display(k.abs(), prefs.system), 0);
        let unit = prefs.labels.distance;
        let key = if k < 0.0 {
            "garage.dist_over"
        } else {
            "garage.dist_left"
        };
        parts.push(tf(key, &[("v", &v), ("unit", &unit)]));
    } else if let Some(due) = item.due_km {
        parts.push(tf("garage.at", &[("v", &fmt_km(Some(due), prefs))]));
    }
    if parts.is_empty() {
        t("garage.set_baseline").into()
    } else {
        parts.join(" · ")
    }
}

fn interval_label(item: &MaintenanceItem, prefs: &UnitPrefs) -> String {
    let mut parts = Vec::new();
    if let Some(km) = item.interval_km {
        parts.push(tf("garage.every", &[("v", &fmt_km(Some(km), prefs))]));
    }
    if let Some(m) = item.interval_months {
        parts.push(tf("garage.every_months", &[("n", &m)]));
    }
    if parts.is_empty() {
        "—".into()
    } else {
        parts.join(" / ")
    }
}

fn last_done_label(item: &MaintenanceItem, prefs: &UnitPrefs) -> String {
    match (&item.last_done_on, item.last_done_km) {
        (Some(d), Some(km)) => {
            format!("{} · {}", crate::i18n::iso_date(d), fmt_km(Some(km), prefs))
        }
        (Some(d), None) => crate::i18n::iso_date(d),
        (None, Some(km)) => fmt_km(Some(km), prefs),
        (None, None) => "—".into(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GarageTab {
    Maintenance,
    Fuel,
}

/// Everything the garage shows for one car, loaded together.
#[derive(Clone, Default, PartialEq)]
struct GarageData {
    items: Vec<MaintenanceItem>,
    log: Vec<MaintenanceLogEntry>,
    due: Option<DueResponse>,
    fuel: Vec<FuelEntry>,
    summary: Option<FuelSummary>,
}

#[component]
pub fn GarageSection(
    /// Car id; empty until the route resolves.
    #[prop(into)]
    car_id: Signal<String>,
    /// Owner or editor: show the forms.
    #[prop(into)]
    can_edit: Signal<bool>,
    /// Full-electric car: charging defaults to kWh.
    #[prop(into)]
    electric: Signal<bool>,
) -> impl IntoView {
    let prefs = use_unit_prefs();
    let tab = RwSignal::new(GarageTab::Maintenance);
    let data = RwSignal::new(GarageData::default());
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);
    let refresh = RwSignal::new(0u32);
    let busy = RwSignal::new(false);

    Effect::new(move |_| {
        let id = car_id.get();
        refresh.track();
        if id.is_empty() {
            return;
        }
        loading.set(true);
        leptos::task::spawn_local(async move {
            let mut next = GarageData::default();
            let mut err: Option<String> = None;
            match list_maintenance_items(&id).await {
                Ok(v) => next.items = v,
                Err(e) => err = Some(e.to_string()),
            }
            match list_maintenance_log(&id).await {
                Ok(v) => next.log = v,
                Err(e) => err = err.or(Some(e.to_string())),
            }
            next.due = maintenance_due(&id).await.ok();
            match list_fuel_log(&id).await {
                Ok(v) => next.fuel = v,
                Err(e) => err = err.or(Some(e.to_string())),
            }
            next.summary = fuel_summary(&id).await.ok();
            // A late response for a car the page has left must not land.
            if car_id.try_get_untracked().as_deref() != Some(id.as_str()) {
                return;
            }
            data.set(next);
            error.set(err);
            loading.set(false);
        });
    });

    // Run one mutation, then reload the whole garage (due dates and the summary
    // depend on every list).
    let mutate =
        move |fut: std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>>>>| {
            busy.set(true);
            error.set(None);
            leptos::task::spawn_local(async move {
                if let Err(e) = fut.await {
                    let _ = error.try_set(Some(e));
                }
                let _ = busy.try_set(false);
                refresh.update(|n| *n = n.wrapping_add(1));
            });
        };

    view! {
        <section class="card garage-card">
            <div class="telemetry-section-head garage-head">
                <h2 class="section-title">
                    <Icon name="wrench" color=IconColor::Warn />
                    {tr!("garage.title")}
                </h2>
                <div class="seg-control" role="tablist" aria-label=tr!("garage.sections")>
                    <button
                        type="button"
                        role="tab"
                        class=move || if tab.get() == GarageTab::Maintenance { "seg-btn is-active" } else { "seg-btn" }
                        aria-selected=move || (tab.get() == GarageTab::Maintenance).to_string()
                        on:click=move |_| tab.set(GarageTab::Maintenance)
                    >
                        {tr!("garage.maintenance")}
                    </button>
                    <button
                        type="button"
                        role="tab"
                        class=move || if tab.get() == GarageTab::Fuel { "seg-btn is-active" } else { "seg-btn" }
                        aria-selected=move || (tab.get() == GarageTab::Fuel).to_string()
                        on:click=move |_| tab.set(GarageTab::Fuel)
                    >
                        {move || if electric.get() { t("garage.charging") } else { t("garage.fuel_charging") }}
                    </button>
                </div>
            </div>
            <Show when=move || error.get().is_some()>
                <div class="error">{move || error.get().unwrap_or_default()}</div>
            </Show>
            <Show when=move || loading.get() && data.with(|d| d.items.is_empty() && d.fuel.is_empty())>
                <div class="empty-state compact">
                    <Icon name="spinner-gap" size=IconSize::Lg color=IconColor::Accent />
                    <div>{tr!("garage.loading")}</div>
                </div>
            </Show>
            <Show
                when=move || tab.get() == GarageTab::Maintenance
                fallback=move || view! {
                    <FuelPanel car_id=car_id can_edit=can_edit electric=electric data=data busy=busy mutate=mutate prefs=prefs />
                }
            >
                <MaintenancePanel car_id=car_id can_edit=can_edit data=data busy=busy mutate=mutate prefs=prefs />
            </Show>
        </section>
    }
}

#[component]
fn MaintenancePanel(
    car_id: Signal<String>,
    can_edit: Signal<bool>,
    data: RwSignal<GarageData>,
    busy: RwSignal<bool>,
    mutate: impl Fn(std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>>>>)
    + Copy
    + Send
    + Sync
    + 'static,
    prefs: RwSignal<UnitPrefs>,
) -> impl IntoView {
    // Odometer reading.
    let odo_input = RwSignal::new(String::new());
    // Schedule item form (add, or edit when `editing` holds an id).
    let editing = RwSignal::new(Option::<String>::None);
    let item_name = RwSignal::new(String::new());
    let item_km = RwSignal::new(String::new());
    let item_months = RwSignal::new(String::new());
    let item_last_on = RwSignal::new(String::new());
    let item_last_km = RwSignal::new(String::new());
    let item_notes = RwSignal::new(String::new());
    // Service log form.
    let log_item = RwSignal::new(String::new());
    let log_date = RwSignal::new(today_input());
    let log_odo = RwSignal::new(String::new());
    let log_title = RwSignal::new(String::new());
    let log_cost = RwSignal::new(String::new());
    let log_currency = RwSignal::new(String::new());
    let log_workshop = RwSignal::new(String::new());
    let log_notes = RwSignal::new(String::new());

    let clear_item_form = move || {
        editing.set(None);
        item_name.set(String::new());
        item_km.set(String::new());
        item_months.set(String::new());
        item_last_on.set(String::new());
        item_last_km.set(String::new());
        item_notes.set(String::new());
    };

    let save_odo = move |_| {
        let system = prefs.get_untracked().system;
        let Some(v) = parse_num(&odo_input.get_untracked()).filter(|v| *v >= 0.0) else {
            return;
        };
        let id = car_id.get_untracked();
        odo_input.set(String::new());
        mutate(Box::pin(async move {
            add_odometer(&id, display_to_km(v, system))
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        }));
    };

    let save_item = move |_| {
        let system = prefs.get_untracked().system;
        let name = item_name.get_untracked();
        if name.trim().is_empty() {
            return;
        }
        let body = serde_json::json!({
            "name": name.trim(),
            "interval_km": parse_num(&item_km.get_untracked()).map(|v| display_to_km(v, system)),
            "interval_months": parse_num(&item_months.get_untracked()).map(|v| v.round() as i32),
            "last_done_on": opt_text(&item_last_on.get_untracked()),
            "last_done_km": parse_num(&item_last_km.get_untracked()).map(|v| display_to_km(v, system)),
            "notes": opt_text(&item_notes.get_untracked()),
        });
        let id = car_id.get_untracked();
        let edit_id = editing.get_untracked();
        clear_item_form();
        mutate(Box::pin(async move {
            let res = match edit_id {
                Some(item_id) => update_maintenance_item(&id, &item_id, &body).await,
                None => create_maintenance_item(&id, &body).await,
            };
            res.map(|_| ()).map_err(|e| e.to_string())
        }));
    };

    let save_log = move |_| {
        let system = prefs.get_untracked().system;
        let item_id = opt_text(&log_item.get_untracked());
        let title = opt_text(&log_title.get_untracked());
        if item_id.is_none() && title.is_none() {
            return;
        }
        let body = serde_json::json!({
            "item_id": item_id,
            "done_on": opt_text(&log_date.get_untracked()).unwrap_or_else(today_input),
            "odometer_km": parse_num(&log_odo.get_untracked()).map(|v| display_to_km(v, system)),
            "title": title,
            "cost": parse_num(&log_cost.get_untracked()),
            "currency": opt_text(&log_currency.get_untracked()),
            "workshop": opt_text(&log_workshop.get_untracked()),
            "notes": opt_text(&log_notes.get_untracked()),
        });
        let id = car_id.get_untracked();
        log_item.set(String::new());
        log_odo.set(String::new());
        log_title.set(String::new());
        log_cost.set(String::new());
        log_workshop.set(String::new());
        log_notes.set(String::new());
        mutate(Box::pin(async move {
            create_maintenance_log(&id, &body)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        }));
    };

    view! {
        <div class="garage-panel">
            <div class="garage-odo">
                <div>
                    <div class="stat-label">{tr!("garage.current_odometer")}</div>
                    <div class="stat-value">
                        {move || {
                            let p = prefs.get();
                            fmt_km(data.with(|d| d.due.as_ref().and_then(|d| d.odometer_km)), &p)
                        }}
                    </div>
                </div>
                <Show when=move || can_edit.get()>
                    <div class="garage-inline-form">
                        <label class="sr-only" for="garage-odo-input">{tr!("garage.odometer_reading")}</label>
                        <input
                            id="garage-odo-input"
                            type="number"
                            inputmode="decimal"
                            min="0"
                            placeholder=move || tf("garage.reading_unit", &[("unit", &prefs.get().labels.odometer)])
                            prop:value=move || odo_input.get()
                            on:input=move |ev| odo_input.set(event_target_value(&ev))
                        />
                        <button
                            type="button"
                            class="btn secondary btn-sm"
                            prop:disabled=move || busy.get() || parse_num(&odo_input.get()).is_none()
                            on:click=save_odo
                        >
                            <Icon name="gauge" size=IconSize::Sm />
                            {tr!("garage.record")}
                        </button>
                    </div>
                </Show>
            </div>

            <h3 class="garage-subtitle">{tr!("garage.due")}</h3>
            {move || {
                let p = prefs.get();
                let items = data.with(|d| d.due.as_ref().map(|d| d.items.clone()).unwrap_or_default());
                if items.is_empty() {
                    view! {
                        <p class="muted">{tr!("garage.no_schedule")}</p>
                    }
                    .into_any()
                } else {
                    view! {
                        <ul class="garage-due-list">
                            {items
                                .into_iter()
                                .map(|item| {
                                    let (class, label) = due_badge(&item.status);
                                    let detail = due_detail(&item, &p);
                                    // Built inside the reactive block above, so a
                                    // locale change re-renders the list.
                                    let _ = crate::i18n::locale();
                                    view! {
                                        <li class="garage-due-item">
                                            <span class="garage-due-name">{item.name.clone()}</span>
                                            <span class="muted garage-due-detail">{detail}</span>
                                            <span class=class>{label}</span>
                                        </li>
                                    }
                                })
                                .collect_view()}
                        </ul>
                    }
                    .into_any()
                }
            }}

            <h3 class="garage-subtitle">{tr!("garage.schedule")}</h3>
            <div class="table-scroll">
                <table class="table garage-table">
                    <thead>
                        <tr>
                            <th>{tr!("garage.item")}</th>
                            <th>{tr!("garage.interval")}</th>
                            <th>{tr!("garage.last_done")}</th>
                            <th></th>
                        </tr>
                    </thead>
                    <tbody>
                        <For
                            each=move || data.with(|d| d.items.clone())
                            key=|i| format!("{}:{:?}:{:?}:{:?}", i.id, i.interval_km, i.last_done_on, i.last_done_km)
                            children=move |item| {
                                let interval = {
                                    let item = item.clone();
                                    move || interval_label(&item, &prefs.get())
                                };
                                let last = {
                                    let item = item.clone();
                                    move || last_done_label(&item, &prefs.get())
                                };
                                let edit_item = item.clone();
                                let del_id = item.id.clone();
                                let del_name = item.name.clone();
                                view! {
                                    <tr>
                                        <td data-label=tr!("garage.item")>
                                            {item.name.clone()}
                                            {item.notes.clone().map(|n| view! { <div class="muted garage-note">{n}</div> })}
                                        </td>
                                        <td data-label=tr!("garage.interval")>{interval}</td>
                                        <td data-label=tr!("garage.last_done")>{last}</td>
                                        <td data-label="">
                                            <Show when=move || can_edit.get()>
                                                <div class="garage-row-actions">
                                                    <button
                                                        type="button"
                                                        class="btn ghost btn-sm"
                                                        on:click={
                                                            let it = edit_item.clone();
                                                            move |_| {
                                                                let system = prefs.get_untracked().system;
                                                                let fmt = |v: Option<f64>| {
                                                                    v.map(|x| format!("{:.0}", km_to_display(x, system)))
                                                                        .unwrap_or_default()
                                                                };
                                                                editing.set(Some(it.id.clone()));
                                                                item_name.set(it.name.clone());
                                                                item_km.set(fmt(it.interval_km));
                                                                item_months.set(
                                                                    it.interval_months.map(|m| m.to_string()).unwrap_or_default(),
                                                                );
                                                                item_last_on.set(it.last_done_on.clone().unwrap_or_default());
                                                                item_last_km.set(fmt(it.last_done_km));
                                                                item_notes.set(it.notes.clone().unwrap_or_default());
                                                            }
                                                        }
                                                    >
                                                        <Icon name="pencil-simple" size=IconSize::Sm />
                                                        {tr!("common.edit")}
                                                    </button>
                                                    <button
                                                        type="button"
                                                        class="btn ghost btn-sm err"
                                                        prop:disabled=move || busy.get()
                                                        on:click={
                                                            let id = del_id.clone();
                                                            let name = del_name.clone();
                                                            move |_| {
                                                                if !confirm(&tf("garage.confirm_delete_item", &[("name", &name)])) {
                                                                    return;
                                                                }
                                                                let car = car_id.get_untracked();
                                                                let id = id.clone();
                                                                mutate(Box::pin(async move {
                                                                    delete_maintenance_item(&car, &id)
                                                                        .await
                                                                        .map_err(|e| e.to_string())
                                                                }));
                                                            }
                                                        }
                                                    >
                                                        <Icon name="trash" size=IconSize::Sm />
                                                        {tr!("common.delete")}
                                                    </button>
                                                </div>
                                            </Show>
                                        </td>
                                    </tr>
                                }
                            }
                        />
                    </tbody>
                </table>
            </div>

            <Show when=move || can_edit.get()>
                <div class="garage-form">
                    <div class="garage-form-title">
                        {move || if editing.get().is_some() { t("garage.edit_item") } else { t("garage.add_schedule_item") }}
                    </div>
                    <div class="garage-form-grid">
                        <label class="garage-field">
                            <span>{tr!("common.name")}</span>
                            <input type="text" placeholder=tr!("garage.oil_change")
                                prop:value=move || item_name.get()
                                on:input=move |ev| item_name.set(event_target_value(&ev)) />
                        </label>
                        <label class="garage-field">
                            <span>{move || tf("garage.every_unit", &[("unit", &prefs.get().labels.distance)])}</span>
                            <input type="number" inputmode="decimal" min="0" placeholder="15000"
                                prop:value=move || item_km.get()
                                on:input=move |ev| item_km.set(event_target_value(&ev)) />
                        </label>
                        <label class="garage-field">
                            <span>{tr!("garage.every_months_label")}</span>
                            <input type="number" inputmode="numeric" min="1" placeholder="12"
                                prop:value=move || item_months.get()
                                on:input=move |ev| item_months.set(event_target_value(&ev)) />
                        </label>
                        <label class="garage-field">
                            <span>{tr!("garage.last_done_on")}</span>
                            <input type="date"
                                prop:value=move || item_last_on.get()
                                on:input=move |ev| item_last_on.set(event_target_value(&ev)) />
                        </label>
                        <label class="garage-field">
                            <span>{move || tf("garage.last_done_at", &[("unit", &prefs.get().labels.odometer)])}</span>
                            <input type="number" inputmode="decimal" min="0"
                                prop:value=move || item_last_km.get()
                                on:input=move |ev| item_last_km.set(event_target_value(&ev)) />
                        </label>
                        <label class="garage-field garage-field-wide">
                            <span>{tr!("common.notes")}</span>
                            <input type="text" placeholder=tr!("garage.notes_placeholder")
                                prop:value=move || item_notes.get()
                                on:input=move |ev| item_notes.set(event_target_value(&ev)) />
                        </label>
                    </div>
                    <div class="row">
                        <button
                            type="button"
                            class="btn primary btn-sm"
                            prop:disabled=move || busy.get() || item_name.get().trim().is_empty()
                            on:click=save_item
                        >
                            <Icon name="floppy-disk" size=IconSize::Sm />
                            {move || if editing.get().is_some() { t("garage.save_item") } else { t("garage.add_item") }}
                        </button>
                        <Show when=move || editing.get().is_some()>
                            <button type="button" class="btn ghost btn-sm" on:click=move |_| clear_item_form()>
                                {tr!("common.cancel")}
                            </button>
                        </Show>
                    </div>
                </div>
            </Show>

            <h3 class="garage-subtitle">{tr!("garage.service_log")}</h3>
            <Show
                when=move || data.with(|d| !d.log.is_empty())
                fallback=|| view! { <p class="muted">{tr!("garage.no_services")}</p> }
            >
                <div class="table-scroll">
                    <table class="table garage-table">
                        <thead>
                            <tr>
                                <th>{tr!("common.date")}</th>
                                <th>{tr!("garage.service")}</th>
                                <th>{tr!("common.odometer")}</th>
                                <th>{tr!("garage.cost")}</th>
                                <th>{tr!("garage.workshop")}</th>
                                <th></th>
                            </tr>
                        </thead>
                        <tbody>
                            <For
                                each=move || data.with(|d| d.log.clone())
                                key=|e| e.id.clone()
                                children=move |e| {
                                    let odo = e.odometer_km;
                                    let odo_label = move || fmt_km(odo, &prefs.get());
                                    let id = e.id.clone();
                                    let done_on = e.done_on.clone();
                                    let (cost, cost_cur) = (e.cost, e.currency.clone());
                                    view! {
                                        <tr>
                                            <td class="num" data-label=tr!("common.date")>{move || crate::i18n::iso_date(&done_on)}</td>
                                            <td data-label=tr!("garage.service")>
                                                {e.title.clone()}
                                                {e.notes.clone().map(|n| view! { <div class="muted garage-note">{n}</div> })}
                                            </td>
                                            <td class="num" data-label=tr!("common.odometer")>{odo_label}</td>
                                            <td class="num" data-label=tr!("garage.cost")>{move || fmt_money(cost, cost_cur.as_deref())}</td>
                                            <td data-label=tr!("garage.workshop")>{e.workshop.clone().unwrap_or_else(|| "—".into())}</td>
                                            <td data-label="">
                                                <Show when=move || can_edit.get()>
                                                    <button
                                                        type="button"
                                                        class="btn ghost btn-sm err"
                                                        aria-label=tr!("garage.delete_service")
                                                        prop:disabled=move || busy.get()
                                                        on:click={
                                                            let id = id.clone();
                                                            move |_| {
                                                                if !confirm(t("garage.confirm_delete_service")) {
                                                                    return;
                                                                }
                                                                let car = car_id.get_untracked();
                                                                let id = id.clone();
                                                                mutate(Box::pin(async move {
                                                                    delete_maintenance_log(&car, &id)
                                                                        .await
                                                                        .map_err(|e| e.to_string())
                                                                }));
                                                            }
                                                        }
                                                    >
                                                        <Icon name="trash" size=IconSize::Sm />
                                                    </button>
                                                </Show>
                                            </td>
                                        </tr>
                                    }
                                }
                            />
                        </tbody>
                    </table>
                </div>
            </Show>

            <Show when=move || can_edit.get()>
                <div class="garage-form">
                    <div class="garage-form-title">{tr!("garage.log_service")}</div>
                    <div class="garage-form-grid">
                        <label class="garage-field">
                            <span>{tr!("garage.schedule_item")}</span>
                            <select
                                prop:value=move || log_item.get()
                                on:change=move |ev| log_item.set(event_target_value(&ev))
                            >
                                <option value="">{tr!("garage.one_off")}</option>
                                <For
                                    each=move || data.with(|d| d.items.clone())
                                    key=|i| i.id.clone()
                                    children=move |i| view! { <option value=i.id.clone()>{i.name.clone()}</option> }
                                />
                            </select>
                        </label>
                        <label class="garage-field">
                            <span>{tr!("garage.title_label")}</span>
                            <input type="text"
                                placeholder=move || if log_item.get().is_empty() { t("garage.brake_pads") } else { t("garage.defaults_item_name") }
                                prop:value=move || log_title.get()
                                on:input=move |ev| log_title.set(event_target_value(&ev)) />
                        </label>
                        <label class="garage-field">
                            <span>{tr!("common.date")}</span>
                            <input type="date"
                                prop:value=move || log_date.get()
                                on:input=move |ev| log_date.set(event_target_value(&ev)) />
                        </label>
                        <label class="garage-field">
                            <span>{move || tf("garage.odometer_unit", &[("unit", &prefs.get().labels.odometer)])}</span>
                            <input type="number" inputmode="decimal" min="0"
                                prop:value=move || log_odo.get()
                                on:input=move |ev| log_odo.set(event_target_value(&ev)) />
                        </label>
                        <label class="garage-field">
                            <span>{tr!("garage.cost")}</span>
                            <input type="number" inputmode="decimal" min="0" step="0.01"
                                prop:value=move || log_cost.get()
                                on:input=move |ev| log_cost.set(event_target_value(&ev)) />
                        </label>
                        <label class="garage-field">
                            <span>{tr!("garage.currency")}</span>
                            <input type="text" maxlength="8" placeholder="EUR"
                                prop:value=move || log_currency.get()
                                on:input=move |ev| log_currency.set(event_target_value(&ev)) />
                        </label>
                        <label class="garage-field">
                            <span>{tr!("garage.workshop")}</span>
                            <input type="text"
                                prop:value=move || log_workshop.get()
                                on:input=move |ev| log_workshop.set(event_target_value(&ev)) />
                        </label>
                        <label class="garage-field garage-field-wide">
                            <span>{tr!("common.notes")}</span>
                            <input type="text"
                                prop:value=move || log_notes.get()
                                on:input=move |ev| log_notes.set(event_target_value(&ev)) />
                        </label>
                    </div>
                    <p class="field-hint">{tr!("garage.log_hint")}</p>
                    <button
                        type="button"
                        class="btn primary btn-sm"
                        prop:disabled=move || {
                            busy.get() || (log_item.get().is_empty() && log_title.get().trim().is_empty())
                        }
                        on:click=save_log
                    >
                        <Icon name="plus" size=IconSize::Sm />
                        {tr!("garage.add_to_log")}
                    </button>
                </div>
            </Show>
        </div>
    }
}

#[component]
fn FuelPanel(
    car_id: Signal<String>,
    can_edit: Signal<bool>,
    electric: Signal<bool>,
    data: RwSignal<GarageData>,
    busy: RwSignal<bool>,
    mutate: impl Fn(std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>>>>)
    + Copy
    + Send
    + Sync
    + 'static,
    prefs: RwSignal<UnitPrefs>,
) -> impl IntoView {
    let unit = RwSignal::new(if electric.get_untracked() { "kWh" } else { "L" }.to_string());
    let filled_at = RwSignal::new(String::new());
    let odo = RwSignal::new(String::new());
    let quantity = RwSignal::new(String::new());
    let price = RwSignal::new(String::new());
    let total = RwSignal::new(String::new());
    let currency = RwSignal::new(String::new());
    let full_tank = RwSignal::new(true);
    let station = RwSignal::new(String::new());
    let notes = RwSignal::new(String::new());

    // Prefill the currency from the log once it is known.
    Effect::new(move |_| {
        let c = data.with(|d| d.summary.as_ref().and_then(|s| s.currency.clone()));
        if let Some(c) = c
            && currency.get_untracked().is_empty()
        {
            currency.set(c);
        }
    });

    // Volume label for the current unit: kWh stays kWh; litres follow the unit system.
    let qty_label = move || {
        if unit.get() == "kWh" {
            "kWh".to_string()
        } else {
            prefs.get().labels.fuel_volume.to_string()
        }
    };

    let save = move |_| {
        let system = prefs.get_untracked().system;
        let is_l = unit.get_untracked() != "kWh";
        let Some(q) = parse_num(&quantity.get_untracked()).filter(|v| *v > 0.0) else {
            return;
        };
        let body = serde_json::json!({
            "filled_at": local_input_to_rfc3339(&filled_at.get_untracked()),
            "odometer_km": parse_num(&odo.get_untracked()).map(|v| display_to_km(v, system)),
            "unit": if is_l { "L" } else { "kWh" },
            "quantity": if is_l { display_to_litres(q, system) } else { q },
            "price_per_unit": parse_num(&price.get_untracked()).map(|p| {
                if is_l { display_to_price_per_litre(p, system) } else { p }
            }),
            "total_cost": parse_num(&total.get_untracked()),
            "currency": opt_text(&currency.get_untracked()),
            "full_tank": full_tank.get_untracked(),
            "station": opt_text(&station.get_untracked()),
            "notes": opt_text(&notes.get_untracked()),
        });
        let id = car_id.get_untracked();
        filled_at.set(String::new());
        odo.set(String::new());
        quantity.set(String::new());
        price.set(String::new());
        total.set(String::new());
        station.set(String::new());
        notes.set(String::new());
        mutate(Box::pin(async move {
            create_fuel_entry(&id, &body)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        }));
    };

    view! {
        <div class="garage-panel">
            {move || {
                let p = prefs.get();
                let s = data.with(|d| d.summary.clone()).unwrap_or_default();
                let cur = s.currency.clone();
                let economy = s
                    .measured_l_per_100km
                    .and_then(|v| l_per_100km_to_display(v, p.system))
                    .map(|v| format!("{} {}", num(v, 1), p.labels.fuel_economy))
                    .unwrap_or_else(|| "—".into());
                let kwh_economy = s
                    .measured_kwh_per_100km
                    .map(|v| match p.system {
                        UnitSystem::Metric => format!("{} kWh/100km", num(v, 1)),
                        UnitSystem::Us => format!("{} kWh/100mi", num(v * crate::units::KM_PER_MILE, 1)),
                    })
                    .unwrap_or_else(|| "—".into());
                let per_dist = s.cost_per_km.map(|c| match p.system {
                    UnitSystem::Metric => c,
                    UnitSystem::Us => c * crate::units::KM_PER_MILE,
                });
                let price_l = s
                    .latest_price_per_l
                    .map(|v| price_per_litre_to_display(v, p.system));
                let tiles = vec![
                    (t("garage.entries"), crate::i18n::int(s.entries as i64)),
                    (
                        t("common.fuel"),
                        if s.total_quantity_l > 0.0 {
                            format!("{} {}", num(litres_to_display(s.total_quantity_l, p.system), 1), p.labels.fuel_volume)
                        } else {
                            "—".into()
                        },
                    ),
                    (
                        t("garage.energy"),
                        if s.total_quantity_kwh > 0.0 {
                            format!("{} kWh", num(s.total_quantity_kwh, 1))
                        } else {
                            "—".into()
                        },
                    ),
                    (t("garage.total_cost"), fmt_money(Some(s.total_cost).filter(|v| *v > 0.0), cur.as_deref())),
                    (t("garage.measured_economy"), economy),
                    (t("garage.measured_energy"), kwh_economy),
                    (
                        if p.system == UnitSystem::Us { t("garage.cost_per_mi") } else { t("garage.cost_per_km") },
                        per_dist.map(|v| num(v, 3)).unwrap_or_else(|| "—".into()),
                    ),
                    (
                        if p.system == UnitSystem::Us { t("garage.price_per_gal") } else { t("garage.price_per_l") },
                        fmt_money(price_l, cur.as_deref()),
                    ),
                    (t("garage.price_per_kwh"), fmt_money(s.latest_price_per_kwh, cur.as_deref())),
                    ("CO₂", s.co2_kg.map(|v| format!("{} kg", num(v, 0))).unwrap_or_else(|| "—".into())),
                ];
                view! {
                    <div class="garage-kpis">
                        {tiles
                            .into_iter()
                            .map(|(label, value)| view! {
                                <div class="metric-chip">
                                    <span class="metric-chip-label">{label}</span>
                                    <span class="metric-chip-value">{value}</span>
                                </div>
                            })
                            .collect_view()}
                    </div>
                }
            }}
            <p class="field-hint">{tr!("garage.economy_hint")}</p>

            <Show when=move || can_edit.get()>
                <div class="garage-form">
                    <div class="garage-form-title">{move || if electric.get() { t("garage.log_charge") } else { t("garage.log_fill") }}</div>
                    <div class="garage-form-grid">
                        <label class="garage-field">
                            <span>{tr!("garage.type")}</span>
                            <select prop:value=move || unit.get() on:change=move |ev| unit.set(event_target_value(&ev))>
                                <option value="L">{tr!("common.fuel")}</option>
                                <option value="kWh">{tr!("garage.charge_kwh")}</option>
                            </select>
                        </label>
                        <label class="garage-field">
                            <span>{tr!("garage.when")}</span>
                            <input type="datetime-local"
                                prop:value=move || filled_at.get()
                                on:input=move |ev| filled_at.set(event_target_value(&ev)) />
                        </label>
                        <label class="garage-field">
                            <span>{move || tf("garage.quantity_unit", &[("unit", &qty_label())])}</span>
                            <input type="number" inputmode="decimal" min="0" step="0.01"
                                prop:value=move || quantity.get()
                                on:input=move |ev| quantity.set(event_target_value(&ev)) />
                        </label>
                        <label class="garage-field">
                            <span>{move || tf("garage.price_unit", &[("unit", &qty_label())])}</span>
                            <input type="number" inputmode="decimal" min="0" step="0.001"
                                prop:value=move || price.get()
                                on:input=move |ev| price.set(event_target_value(&ev)) />
                        </label>
                        <label class="garage-field">
                            <span>{tr!("garage.total_cost")}</span>
                            <input type="number" inputmode="decimal" min="0" step="0.01"
                                prop:value=move || total.get()
                                on:input=move |ev| total.set(event_target_value(&ev)) />
                        </label>
                        <label class="garage-field">
                            <span>{tr!("garage.currency")}</span>
                            <input type="text" maxlength="8" placeholder="EUR"
                                prop:value=move || currency.get()
                                on:input=move |ev| currency.set(event_target_value(&ev)) />
                        </label>
                        <label class="garage-field">
                            <span>{move || tf("garage.odometer_unit", &[("unit", &prefs.get().labels.odometer)])}</span>
                            <input type="number" inputmode="decimal" min="0"
                                prop:value=move || odo.get()
                                on:input=move |ev| odo.set(event_target_value(&ev)) />
                        </label>
                        <label class="garage-field">
                            <span>{tr!("garage.station")}</span>
                            <input type="text"
                                prop:value=move || station.get()
                                on:input=move |ev| station.set(event_target_value(&ev)) />
                        </label>
                        <label class="garage-field garage-field-wide">
                            <span>{tr!("common.notes")}</span>
                            <input type="text"
                                prop:value=move || notes.get()
                                on:input=move |ev| notes.set(event_target_value(&ev)) />
                        </label>
                        <label class="garage-check">
                            <input type="checkbox"
                                prop:checked=move || full_tank.get()
                                on:change=move |ev| full_tank.set(event_target_checked(&ev)) />
                            <span>{move || if unit.get() == "kWh" { t("garage.charged_full") } else { t("garage.filled_full") }}</span>
                        </label>
                    </div>
                    <p class="field-hint">{tr!("garage.fill_hint")}</p>
                    <button
                        type="button"
                        class="btn primary btn-sm"
                        prop:disabled=move || busy.get() || parse_num(&quantity.get()).is_none_or(|v| v <= 0.0)
                        on:click=save
                    >
                        <Icon name="plus" size=IconSize::Sm />
                        {tr!("garage.add_entry")}
                    </button>
                </div>
            </Show>

            <h3 class="garage-subtitle">{tr!("garage.log")}</h3>
            <Show
                when=move || data.with(|d| !d.fuel.is_empty())
                fallback=|| view! { <p class="muted">{tr!("garage.nothing_logged")}</p> }
            >
                <div class="table-scroll">
                    <table class="table garage-table">
                        <thead>
                            <tr>
                                <th>{tr!("garage.when")}</th>
                                <th>{tr!("garage.quantity")}</th>
                                <th>{tr!("garage.price")}</th>
                                <th>{tr!("garage.total")}</th>
                                <th>{tr!("common.odometer")}</th>
                                <th>{tr!("garage.station")}</th>
                                <th></th>
                            </tr>
                        </thead>
                        <tbody>
                            <For
                                each=move || data.with(|d| d.fuel.clone())
                                key=|e| e.id.clone()
                                children=move |e| {
                                    let e2 = e.clone();
                                    let qty = move || {
                                        let p = prefs.get();
                                        let full = if e2.full_tank { t("garage.full_suffix") } else { "" };
                                        if e2.unit == "kWh" {
                                            format!("{} kWh{full}", num(e2.quantity, 2))
                                        } else {
                                            format!(
                                                "{} {}{full}",
                                                num(litres_to_display(e2.quantity, p.system), 2),
                                                p.labels.fuel_volume
                                            )
                                        }
                                    };
                                    let e3 = e.clone();
                                    let price_label = move || {
                                        let p = prefs.get();
                                        let v = if e3.unit == "kWh" {
                                            e3.price_per_unit
                                        } else {
                                            e3.price_per_unit.map(|x| price_per_litre_to_display(x, p.system))
                                        };
                                        fmt_money(v, e3.currency.as_deref())
                                    };
                                    let odo_km = e.odometer_km;
                                    let odo_label = move || fmt_km(odo_km, &prefs.get());
                                    let id = e.id.clone();
                                    let filled_at = e.filled_at.clone();
                                    let (total, total_cur) = (e.total_cost, e.currency.clone());
                                    view! {
                                        <tr>
                                            <td class="num" data-label=tr!("garage.when")>{move || local_time(&filled_at)}</td>
                                            <td class="num" data-label=tr!("garage.quantity")>{qty}</td>
                                            <td class="num" data-label=tr!("garage.price")>{price_label}</td>
                                            <td class="num" data-label=tr!("garage.total")>{move || fmt_money(total, total_cur.as_deref())}</td>
                                            <td class="num" data-label=tr!("common.odometer")>{odo_label}</td>
                                            <td data-label=tr!("garage.station")>{e.station.clone().unwrap_or_else(|| "—".into())}</td>
                                            <td data-label="">
                                                <Show when=move || can_edit.get()>
                                                    <button
                                                        type="button"
                                                        class="btn ghost btn-sm err"
                                                        aria-label=tr!("garage.delete_entry")
                                                        prop:disabled=move || busy.get()
                                                        on:click={
                                                            let id = id.clone();
                                                            move |_| {
                                                                if !confirm(t("garage.confirm_delete_entry")) {
                                                                    return;
                                                                }
                                                                let car = car_id.get_untracked();
                                                                let id = id.clone();
                                                                mutate(Box::pin(async move {
                                                                    delete_fuel_entry(&car, &id)
                                                                        .await
                                                                        .map_err(|e| e.to_string())
                                                                }));
                                                            }
                                                        }
                                                    >
                                                        <Icon name="trash" size=IconSize::Sm />
                                                    </button>
                                                </Show>
                                            </td>
                                        </tr>
                                    }
                                }
                            />
                        </tbody>
                    </table>
                </div>
            </Show>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_accept_a_decimal_comma() {
        assert_eq!(parse_num("1,5"), Some(1.5));
        assert_eq!(parse_num(" 42 "), Some(42.0));
        assert_eq!(parse_num(""), None);
        assert_eq!(parse_num("abc"), None);
    }

    #[test]
    fn due_detail_reads_days_and_distance() {
        let item = DueItem {
            item_id: "i".into(),
            name: "Oil".into(),
            due_on: Some("2026-10-01".into()),
            due_km: Some(15000.0),
            days_left: Some(-3),
            km_left: Some(500.0),
            status: "overdue".into(),
        };
        let s = due_detail(&item, &UnitPrefs::default());
        assert_eq!(s, "2026-10-01 · 3 days late · 500 km left");
        let es = crate::i18n::with_locale(crate::i18n::Locale::Es, || {
            due_detail(
                &DueItem {
                    km_left: Some(-1500.0),
                    ..item.clone()
                },
                &UnitPrefs::default(),
            )
        });
        assert_eq!(es, "01/10/2026 · 3 días de retraso · 1.500 km de más");
    }
}
