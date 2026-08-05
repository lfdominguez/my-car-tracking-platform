use leptos::prelude::*;

use crate::api::{get_me, update_me_preferences, update_me_unit_system};
use crate::components::{Icon, IconColor, IconSize};
use crate::units::{use_unit_prefs, UnitPrefs, UnitSystem};

#[component]
pub fn SettingsPage() -> impl IntoView {
    let prefs = use_unit_prefs();
    let saving = RwSignal::new(false);
    let message = RwSignal::new(Option::<String>::None);
    let error = RwSignal::new(Option::<String>::None);

    let openrouter_model = RwSignal::new(String::from("anthropic/claude-3.7-sonnet"));
    let openrouter_key = RwSignal::new(String::new());
    let key_set = RwSignal::new(false);
    let key_hint = RwSignal::new(Option::<String>::None);
    let ors_key = RwSignal::new(String::new());
    let ors_key_set = RwSignal::new(false);
    let ors_key_hint = RwSignal::new(Option::<String>::None);
    let loaded = RwSignal::new(false);

    Effect::new(move |_| {
        if loaded.get() {
            return;
        }
        leptos::task::spawn_local(async move {
            match get_me().await {
                Ok(me) => {
                    openrouter_model.set(me.openrouter_model.clone());
                    key_set.set(me.openrouter_api_key_set);
                    key_hint.set(me.openrouter_api_key_hint.clone());
                    ors_key_set.set(me.ors_api_key_set);
                    ors_key_hint.set(me.ors_api_key_hint.clone());
                    prefs.set(UnitPrefs::from_me(&me));
                    loaded.set(true);
                }
                Err(e) => {
                    error.set(Some(e.to_string()));
                    loaded.set(true);
                }
            }
        });
    });

    let save_units = move |system: UnitSystem| {
        saving.set(true);
        message.set(None);
        error.set(None);
        leptos::task::spawn_local(async move {
            match update_me_unit_system(system.as_str()).await {
                Ok(me) => {
                    prefs.set(UnitPrefs::from_me(&me));
                    message.set(Some(match system {
                        UnitSystem::Metric => "Using International (metric) units.".into(),
                        UnitSystem::Us => "Using Imperial units.".into(),
                    }));
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            saving.set(false);
        });
    };

    let save_openrouter = move |_| {
        saving.set(true);
        message.set(None);
        error.set(None);
        let model = openrouter_model.get();
        let key = openrouter_key.get();
        leptos::task::spawn_local(async move {
            let mut body = serde_json::json!({ "openrouter_model": model });
            if !key.is_empty() {
                body["openrouter_api_key"] = serde_json::json!(key);
            }
            match update_me_preferences(body).await {
                Ok(me) => {
                    openrouter_model.set(me.openrouter_model.clone());
                    key_set.set(me.openrouter_api_key_set);
                    key_hint.set(me.openrouter_api_key_hint.clone());
                    openrouter_key.set(String::new());
                    message.set(Some("OpenRouter settings saved.".into()));
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            saving.set(false);
        });
    };

    let clear_key = move |_| {
        saving.set(true);
        message.set(None);
        error.set(None);
        leptos::task::spawn_local(async move {
            let body = serde_json::json!({ "openrouter_api_key": "" });
            match update_me_preferences(body).await {
                Ok(me) => {
                    key_set.set(me.openrouter_api_key_set);
                    key_hint.set(me.openrouter_api_key_hint.clone());
                    message.set(Some("OpenRouter API key cleared.".into()));
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            saving.set(false);
        });
    };

    view! {
        <div class="page-header">
            <h1>"Settings"</h1>
            <p class="muted">"Units, routing, and AI analysis preferences"</p>
        </div>

        <Show when=move || message.get().is_some()>
            <div class="banner ok">{move || message.get().unwrap_or_default()}</div>
        </Show>
        <Show when=move || error.get().is_some()>
            <div class="banner err">{move || error.get().unwrap_or_default()}</div>
        </Show>

        <div class="card settings-card">
            <h2 class="section-title">
                <Icon name="globe-hemisphere-west" color=IconColor::Accent />
                "Display units"
            </h2>
            <p class="muted">
                "Distance, speed, odometer, fuel volume, and economy on the dashboard and trips. "
                "Raw OBD data stays metric in the database."
            </p>
            <div class="unit-choice-grid" role="radiogroup" aria-label="Unit system">
                <button
                    type="button"
                    class=move || {
                        if prefs.get().system == UnitSystem::Metric {
                            "unit-choice active"
                        } else {
                            "unit-choice"
                        }
                    }
                    prop:disabled=move || saving.get()
                    on:click=move |_| save_units(UnitSystem::Metric)
                >
                    <div class="unit-choice-title">"International"</div>
                    <div class="unit-choice-meta">"km, km/h, L, L/100km"</div>
                </button>
                <button
                    type="button"
                    class=move || {
                        if prefs.get().system == UnitSystem::Us {
                            "unit-choice active"
                        } else {
                            "unit-choice"
                        }
                    }
                    prop:disabled=move || saving.get()
                    on:click=move |_| save_units(UnitSystem::Us)
                >
                    <div class="unit-choice-title">"Imperial"</div>
                    <div class="unit-choice-meta">"mi, mph, gal, mpg"</div>
                </button>
            </div>
        </div>

        <div class="card settings-card" style="margin-top:1rem">
            <h2 class="section-title">
                <Icon name="robot" color=IconColor::Accent />
                "AI route analysis (OpenRouter)"
            </h2>
            <p class="muted">
                "Your personal OpenRouter API key is stored encrypted and used only when you run "
                "Analyze on a trip you own. Usage is billed to your OpenRouter account."
            </p>
            <label class="field">
                <span>"Model id"</span>
                <input
                    type="text"
                    prop:value=move || openrouter_model.get()
                    on:input=move |ev| openrouter_model.set(event_target_value(&ev))
                    placeholder="anthropic/claude-3.7-sonnet"
                />
            </label>
            <label class="field">
                <span>"API key"</span>
                <input
                    type="password"
                    prop:value=move || openrouter_key.get()
                    on:input=move |ev| openrouter_key.set(event_target_value(&ev))
                    placeholder=move || {
                        if key_set.get() {
                            format!("Key saved {}", key_hint.get().unwrap_or_default())
                        } else {
                            "sk-or-…".into()
                        }
                    }
                    autocomplete="off"
                />
            </label>
            <div class="row-actions" style="display:flex;gap:0.5rem;flex-wrap:wrap;margin-top:0.75rem">
                <button type="button" class="btn primary" prop:disabled=move || saving.get() on:click=save_openrouter>
                    "Save OpenRouter"
                </button>
                <Show when=move || key_set.get()>
                    <button type="button" class="btn ghost" prop:disabled=move || saving.get() on:click=clear_key>
                        "Clear key"
                    </button>
                </Show>
            </div>
            <Show when=move || key_set.get()>
                <p class="muted" style="margin-top:0.5rem">
                    "Key on file: " {move || key_hint.get().unwrap_or_else(|| "…".into())}
                </p>
            </Show>
        </div>

        <div class="card settings-card" style="margin-top:1rem">
            <h2 class="section-title">
                <Icon name="map-trifold" color=IconColor::Accent />
                "OpenRouteService"
            </h2>
            <p class="muted">
                "Free API key for Routes Optimization (alternate paths + elevation). "
                "Get a key at openrouteservice.org. Stored encrypted; never shown again in full."
            </p>
            <label class="field">
                <span>"API key"</span>
                <input
                    type="password"
                    prop:value=move || ors_key.get()
                    on:input=move |ev| ors_key.set(event_target_value(&ev))
                    placeholder=move || {
                        if ors_key_set.get() {
                            format!("Key saved {}", ors_key_hint.get().unwrap_or_default())
                        } else {
                            "Paste OpenRouteService key".into()
                        }
                    }
                    autocomplete="off"
                />
            </label>
            <div class="row-actions" style="display:flex;gap:0.5rem;flex-wrap:wrap;margin-top:0.75rem">
                <button
                    type="button"
                    class="btn primary"
                    prop:disabled=move || saving.get()
                    on:click=move |_| {
                        saving.set(true);
                        message.set(None);
                        error.set(None);
                        let key = ors_key.get();
                        leptos::task::spawn_local(async move {
                            if key.is_empty() {
                                message.set(Some(
                                    "Enter a new key to save, or use Clear key.".into(),
                                ));
                                saving.set(false);
                                return;
                            }
                            let body = serde_json::json!({ "ors_api_key": key });
                            match update_me_preferences(body).await {
                                Ok(me) => {
                                    ors_key_set.set(me.ors_api_key_set);
                                    ors_key_hint.set(me.ors_api_key_hint.clone());
                                    ors_key.set(String::new());
                                    message.set(Some("OpenRouteService key saved.".into()));
                                }
                                Err(e) => error.set(Some(e.to_string())),
                            }
                            saving.set(false);
                        });
                    }
                >
                    "Save ORS key"
                </button>
                <Show when=move || ors_key_set.get()>
                    <button
                        type="button"
                        class="btn ghost"
                        prop:disabled=move || saving.get()
                        on:click=move |_| {
                            saving.set(true);
                            message.set(None);
                            error.set(None);
                            leptos::task::spawn_local(async move {
                                let body = serde_json::json!({ "ors_api_key": "" });
                                match update_me_preferences(body).await {
                                    Ok(me) => {
                                        ors_key_set.set(me.ors_api_key_set);
                                        ors_key_hint.set(me.ors_api_key_hint.clone());
                                        message.set(Some("OpenRouteService key cleared.".into()));
                                    }
                                    Err(e) => error.set(Some(e.to_string())),
                                }
                                saving.set(false);
                            });
                        }
                    >
                        "Clear key"
                    </button>
                </Show>
            </div>
            <Show when=move || ors_key_set.get()>
                <p class="muted" style="margin-top:0.5rem">
                    "Key on file: " {move || ors_key_hint.get().unwrap_or_else(|| "…".into())}
                </p>
            </Show>
        </div>

        <Show when=move || saving.get()>
            <div class="muted" style="margin-top:0.75rem">
                <Icon name="spinner-gap" size=IconSize::Sm color=IconColor::Accent />
                " Saving…"
            </div>
        </Show>
    }
}
