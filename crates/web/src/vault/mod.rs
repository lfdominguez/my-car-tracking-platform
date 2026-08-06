//! Client-side vault unlock state and helpers (WASM).

mod ops;

pub use ops::{
    build_analysis_context_json, decrypt_ai_report, decrypt_car_profile, decrypt_track_meta,
    decrypt_track_points, load_car_dek, migrate_all_owned, put_car_profile, seal_ai_report,
    wrap_and_upload_dek, CarProfileV1,
};

use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use leptos::prelude::*;
use vault_crypto::{
    generate_recovery_key, identity_from_recovery, public_identity, IdentityPublic, IdentitySecret,
    RecoveryKey,
};
use zeroize::Zeroize;

const LS_DEVICE_IDENTITY: &str = "ctp_vault_identity_sk_b64";

/// In-memory unlocked vault keys for the tab session (Send+Sync for Leptos context).
#[derive(Clone)]
pub struct VaultSession {
    inner: Arc<Mutex<Option<UnlockedVault>>>,
}

pub struct UnlockedVault {
    pub secret: IdentitySecret,
    pub public: IdentityPublic,
}

// IdentitySecret is not Sync; we only touch it on the WASM main thread via Mutex.
unsafe impl Send for UnlockedVault {}
unsafe impl Sync for UnlockedVault {}

impl VaultSession {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(None)),
        }
    }

    pub fn is_unlocked(&self) -> bool {
        self.inner.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    pub fn with_secret<R>(&self, f: impl FnOnce(&IdentitySecret, &IdentityPublic) -> R) -> Option<R> {
        let g = self.inner.lock().ok()?;
        g.as_ref().map(|u| f(&u.secret, &u.public))
    }

    pub fn unlock_from_recovery(&self, recovery: &str) -> Result<(), String> {
        let rk: RecoveryKey = recovery
            .trim()
            .parse()
            .map_err(|_| "Invalid recovery key".to_string())?;
        let secret = identity_from_recovery(&rk);
        let public = public_identity(&secret);
        if let Some(win) = web_sys::window() {
            if let Ok(Some(storage)) = win.local_storage() {
                let b64 = B64.encode(secret.to_bytes());
                let _ = storage.set_item(LS_DEVICE_IDENTITY, &b64);
            }
        }
        if let Ok(mut g) = self.inner.lock() {
            *g = Some(UnlockedVault { secret, public });
        }
        Ok(())
    }

    pub fn try_unlock_from_device_cache(&self) -> bool {
        let Some(win) = web_sys::window() else {
            return false;
        };
        let Ok(Some(storage)) = win.local_storage() else {
            return false;
        };
        let Ok(Some(b64)) = storage.get_item(LS_DEVICE_IDENTITY) else {
            return false;
        };
        let Ok(bytes) = B64.decode(b64.trim()) else {
            return false;
        };
        if bytes.len() != 32 {
            return false;
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        let secret = IdentitySecret::from_bytes(arr);
        arr.zeroize();
        let public = public_identity(&secret);
        if let Ok(mut g) = self.inner.lock() {
            *g = Some(UnlockedVault { secret, public });
        }
        true
    }

    pub fn lock(&self) {
        if let Ok(mut g) = self.inner.lock() {
            *g = None;
        }
    }

    pub fn public_b64(&self) -> Option<String> {
        self.with_secret(|_, pk| B64.encode(pk.as_bytes()))
    }
}

impl Default for VaultSession {
    fn default() -> Self {
        Self::new()
    }
}

pub fn provide_vault_session() -> VaultSession {
    let session = VaultSession::new();
    let _ = session.try_unlock_from_device_cache();
    provide_context(session.clone());
    session
}

pub fn use_vault_session() -> VaultSession {
    expect_context::<VaultSession>()
}

pub struct GeneratedVault {
    pub recovery_grouped: String,
    pub identity_pubkey_b64: String,
}

pub fn generate_vault_identity() -> GeneratedVault {
    let rk = generate_recovery_key();
    let secret = identity_from_recovery(&rk);
    let public = public_identity(&secret);
    GeneratedVault {
        recovery_grouped: rk.to_grouped_string(),
        identity_pubkey_b64: B64.encode(public.as_bytes()),
    }
}

fn event_target_value(ev: &web_sys::Event) -> String {
    use wasm_bindgen::JsCast;
    ev.target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|el| el.value())
        .or_else(|| {
            ev.target()
                .and_then(|t| t.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
                .map(|el| el.value())
        })
        .unwrap_or_default()
}

/// Settings card: status, enable wizard, unlock/lock.
#[component]
pub fn VaultSettingsCard() -> impl IntoView {
    use crate::api::{vault_activate, vault_enable, vault_status, VaultStatus};

    let status = RwSignal::new(Option::<VaultStatus>::None);
    let recovery_shown = RwSignal::new(Option::<String>::None);
    let pending_pubkey = RwSignal::new(Option::<String>::None);
    let ack = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let msg = RwSignal::new(Option::<String>::None);
    let err = RwSignal::new(Option::<String>::None);
    let unlocked = RwSignal::new(use_vault_session().is_unlocked());
    let unlock_input = RwSignal::new(String::new());

    Effect::new(move |_| {
        leptos::task::spawn_local(async move {
            match vault_status().await {
                Ok(s) => status.set(Some(s)),
                Err(e) => err.set(Some(e.to_string())),
            }
        });
    });

    view! {
        <div class="card" style="margin-top:1.25rem">
            <h2 style="margin-top:0">"Zero-knowledge vault"</h2>
            <p class="muted">
                "Optional E2E encryption for trips, car profiles, and AI results. "
                "Server stores ciphertext only. Lose recovery key + devices ⇒ permanent data loss."
            </p>
            {move || err.get().map(|e| view! { <p class="error">{e}</p> })}
            {move || msg.get().map(|m| view! { <p class="ok">{m}</p> })}

            {move || {
                match status.get() {
                    None => view! { <p class="muted">"Loading…"</p> }.into_any(),
                    Some(s) if !s.vault_ui_enabled => view! {
                        <p class="muted">"Vault UI disabled on this server (VAULT_UI_ENABLED)."</p>
                    }.into_any(),
                    Some(s) => view! {
                        <p>
                            "Status: "<strong>{s.vault_status.clone()}</strong>
                            {if s.vault_enabled { " · enabled" } else { "" }}
                            {format!(" · v{} · {} objects", s.vault_identity_version, s.vault_object_count)}
                        </p>
                    }.into_any(),
                }
            }}

            <Show when=move || status.get().map(|s| s.vault_ui_enabled && s.vault_status == "disabled").unwrap_or(false)>
                <Show when=move || recovery_shown.get().is_none()>
                    <button
                        type="button"
                        class="btn primary"
                        on:click=move |_| {
                            err.set(None);
                            let g = generate_vault_identity();
                            recovery_shown.set(Some(g.recovery_grouped));
                            pending_pubkey.set(Some(g.identity_pubkey_b64));
                            ack.set(false);
                        }
                    >
                        "Enable vault…"
                    </button>
                </Show>
                <Show when=move || recovery_shown.get().is_some()>
                    <div style="border:1px solid #c44;padding:1rem;border-radius:8px;margin-top:0.75rem">
                        <p><strong>"Save this recovery key now (shown once):"</strong></p>
                        <pre style="white-space:pre-wrap;word-break:break-all">
                            {move || recovery_shown.get().unwrap_or_default()}
                        </pre>
                        <label style="display:flex;gap:0.5rem;align-items:flex-start">
                            <input
                                type="checkbox"
                                prop:checked=move || ack.get()
                                on:change=move |ev| {
                                    use wasm_bindgen::JsCast;
                                    let c = ev
                                        .target()
                                        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                        .map(|e| e.checked())
                                        .unwrap_or(false);
                                    ack.set(c);
                                }
                            />
                            <span>
                                "I stored the recovery key. I understand the administrator cannot recover my data."
                            </span>
                        </label>
                        <button
                            type="button"
                            class="btn primary"
                            style="margin-top:0.75rem"
                            prop:disabled=move || !ack.get() || busy.get()
                            on:click=move |_| {
                                if !ack.get() {
                                    return;
                                }
                                let Some(pk) = pending_pubkey.get() else {
                                    return;
                                };
                                let recovery = recovery_shown.get().unwrap_or_default();
                                busy.set(true);
                                err.set(None);
                                let sess = use_vault_session();
                                leptos::task::spawn_local(async move {
                                    match vault_enable(&pk, 1).await {
                                        Ok(s) => {
                                            let _ = sess.unlock_from_recovery(&recovery);
                                            unlocked.set(sess.is_unlocked());
                                            status.set(Some(s));
                                            msg.set(Some(
                                                "Vault migrating. Encrypt data on this device, then Activate.".into(),
                                            ));
                                            recovery_shown.set(None);
                                        }
                                        Err(e) => err.set(Some(e.to_string())),
                                    }
                                    busy.set(false);
                                });
                            }
                        >
                            "Confirm and enable"
                        </button>
                    </div>
                </Show>
            </Show>

            <Show when=move || status.get().map(|s| s.vault_status == "migrating").unwrap_or(false)>
                <p class="muted">
                    "This browser will encrypt owned cars and trips, clear server plaintext, then activate the vault. Stay unlocked."
                </p>
                <button
                    type="button"
                    class="btn primary"
                    prop:disabled=move || busy.get() || !unlocked.get()
                    on:click=move |_| {
                        if !unlocked.get() {
                            err.set(Some("Unlock the vault before migrating.".into()));
                            return;
                        }
                        busy.set(true);
                        err.set(None);
                        msg.set(Some("Migrating…".into()));
                        let sess = use_vault_session();
                        leptos::task::spawn_local(async move {
                            match migrate_all_owned(&sess).await {
                                Ok(m) => match vault_activate().await {
                                    Ok(s) => {
                                        status.set(Some(s));
                                        msg.set(Some(format!("{m}. Vault activated.")));
                                    }
                                    Err(e) => err.set(Some(e.to_string())),
                                },
                                Err(e) => err.set(Some(e)),
                            }
                            busy.set(false);
                        });
                    }
                >
                    "Migrate & activate vault"
                </button>
            </Show>

            <Show when=move || status.get().map(|s| s.vault_status != "disabled").unwrap_or(false)>
                <div style="margin-top:0.75rem">
                    <Show when=move || !unlocked.get()>
                        <label>
                            "Unlock with recovery key"
                            <input
                                type="text"
                                style="width:100%"
                                prop:value=move || unlock_input.get()
                                on:input=move |ev| unlock_input.set(event_target_value(&ev))
                            />
                        </label>
                        <button
                            type="button"
                            class="btn primary"
                            style="margin-top:0.5rem"
                            on:click=move |_| {
                                err.set(None);
                                let sess = use_vault_session();
                                match sess.unlock_from_recovery(&unlock_input.get()) {
                                    Ok(()) => {
                                        unlocked.set(true);
                                        msg.set(Some("Vault unlocked on this device.".into()));
                                    }
                                    Err(e) => err.set(Some(e)),
                                }
                            }
                        >
                            "Unlock"
                        </button>
                    </Show>
                    <Show when=move || unlocked.get()>
                        <p class="ok">"Unlocked on this device."</p>
                        <button
                            type="button"
                            class="btn ghost"
                            on:click=move |_| {
                                use_vault_session().lock();
                                unlocked.set(false);
                                msg.set(Some("Vault locked on this tab.".into()));
                            }
                        >
                            "Lock"
                        </button>
                    </Show>
                </div>
            </Show>
        </div>
    }
}

/// Unlock gate for vault-sealed list/detail pages.
#[component]
pub fn VaultUnlockGate(#[prop(into)] message: String) -> impl IntoView {
    let recovery = RwSignal::new(String::new());
    let error = RwSignal::new(Option::<String>::None);
    let unlocked = RwSignal::new(use_vault_session().is_unlocked());

    view! {
        <div class="card" style="max-width: 32rem; margin: 2rem auto;">
            <h2>"Unlock zero-knowledge vault"</h2>
            <p class="muted">{message}</p>
            <label>
                "Recovery key"
                <textarea
                    prop:value=move || recovery.get()
                    on:input=move |ev| recovery.set(event_target_value(&ev))
                    rows="3"
                    style="width:100%"
                    placeholder="XXXX-XXXX-..."
                />
            </label>
            {move || error.get().map(|e| view! { <p class="error">{e}</p> })}
            <button
                type="button"
                class="btn primary"
                style="margin-top:0.75rem"
                on:click=move |_| {
                    error.set(None);
                    match use_vault_session().unlock_from_recovery(&recovery.get()) {
                        Ok(()) => unlocked.set(true),
                        Err(e) => error.set(Some(e)),
                    }
                }
            >
                "Unlock"
            </button>
            <Show when=move || unlocked.get()>
                <p class="ok">"Vault unlocked. Continue browsing cars and trips."</p>
            </Show>
        </div>
    }
}
