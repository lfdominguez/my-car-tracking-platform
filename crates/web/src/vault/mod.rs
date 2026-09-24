//! Client-side vault unlock state and helpers (WASM).

mod ops;

pub use ops::{
    CarProfileV1, build_analysis_context_json, decrypt_ai_report, decrypt_car_profile,
    decrypt_track_meta, decrypt_track_points, load_car_dek, migrate_all_owned, put_car_profile,
    seal_ai_report, wrap_and_upload_dek,
};

use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use leptos::prelude::*;
use vault_crypto::{
    IdentityPublic, IdentitySecret, RecoveryKey, generate_recovery_key, identity_from_recovery,
    public_identity,
};
use zeroize::Zeroize;

const LS_DEVICE_IDENTITY: &str = "ctp_vault_identity_sk_b64";

/// In-memory unlocked vault keys for the tab session (Send+Sync for Leptos context).
///
/// The keys live behind a `Mutex`, which nothing can subscribe to, so the lock state
/// is mirrored into `unlocked`: views gate on [`VaultSession::unlocked`] and re-render
/// when any component unlocks or locks the vault.
#[derive(Clone)]
pub struct VaultSession {
    inner: Arc<Mutex<Option<UnlockedVault>>>,
    unlocked: RwSignal<bool>,
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
            unlocked: RwSignal::new(false),
        }
    }

    /// Untracked check for async code and event handlers.
    pub fn is_unlocked(&self) -> bool {
        self.inner.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    /// Reactive lock state: read it inside a view or effect to re-run on unlock/lock.
    pub fn unlocked(&self) -> Signal<bool> {
        self.unlocked.into()
    }

    fn set_keys(&self, keys: Option<UnlockedVault>) {
        let unlocked = keys.is_some();
        if let Ok(mut g) = self.inner.lock() {
            *g = keys;
        }
        self.unlocked.set(unlocked);
    }

    pub fn with_secret<R>(
        &self,
        f: impl FnOnce(&IdentitySecret, &IdentityPublic) -> R,
    ) -> Option<R> {
        let g = self.inner.lock().ok()?;
        g.as_ref().map(|u| f(&u.secret, &u.public))
    }

    pub fn unlock_from_recovery(&self, recovery: &str) -> Result<(), String> {
        let rk: RecoveryKey = recovery
            .trim()
            .parse()
            .map_err(|_| crate::i18n::t("vault.invalid_key").to_string())?;
        let secret = identity_from_recovery(&rk);
        let public = public_identity(&secret);
        if let Some(win) = web_sys::window()
            && let Ok(Some(storage)) = win.local_storage()
        {
            let b64 = B64.encode(secret.to_bytes());
            let _ = storage.set_item(LS_DEVICE_IDENTITY, &b64);
        }
        self.set_keys(Some(UnlockedVault { secret, public }));
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
        self.set_keys(Some(UnlockedVault { secret, public }));
        true
    }

    pub fn lock(&self) {
        self.set_keys(None);
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
    use crate::api::{VaultStatus, vault_activate, vault_enable, vault_status};

    let status = RwSignal::new(Option::<VaultStatus>::None);
    let recovery_shown = RwSignal::new(Option::<String>::None);
    let pending_pubkey = RwSignal::new(Option::<String>::None);
    let ack = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let msg = RwSignal::new(Option::<String>::None);
    let err = RwSignal::new(Option::<String>::None);
    let unlocked = use_vault_session().unlocked();
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
            <h2 style="margin-top:0">{tr!("vault.title")}</h2>
            <p class="muted">{tr!("vault.lead")}</p>
            {move || err.get().map(|e| view! { <p class="error">{e}</p> })}
            {move || msg.get().map(|m| view! { <p class="ok">{m}</p> })}

            {move || {
                match status.get() {
                    None => view! { <p class="muted">{tr!("common.loading")}</p> }.into_any(),
                    Some(s) if !s.vault_ui_enabled => view! {
                        <p class="muted">{tr!("vault.ui_disabled")}</p>
                    }.into_any(),
                    Some(s) => view! {
                        <p>
                            {tr!("vault.status")}" "<strong>{vault_status_label(&s.vault_status)}</strong>
                            {if s.vault_enabled { crate::i18n::t("vault.enabled_suffix") } else { "" }}
                            {crate::i18n::tf(
                                "vault.objects",
                                &[("version", &s.vault_identity_version), ("n", &s.vault_object_count)],
                            )}
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
                        {tr!("vault.enable")}
                    </button>
                </Show>
                <Show when=move || recovery_shown.get().is_some()>
                    <div style="border:1px solid #c44;padding:1rem;border-radius:8px;margin-top:0.75rem">
                        <p><strong>{tr!("vault.save_key")}</strong></p>
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
                                {tr!("vault.ack")}
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
                                            status.set(Some(s));
                                            msg.set(Some(crate::i18n::t("vault.migrating_msg").into()));
                                            recovery_shown.set(None);
                                        }
                                        Err(e) => err.set(Some(e.to_string())),
                                    }
                                    busy.set(false);
                                });
                            }
                        >
                            {tr!("vault.confirm_enable")}
                        </button>
                    </div>
                </Show>
            </Show>

            <Show when=move || status.get().map(|s| s.vault_status == "migrating").unwrap_or(false)>
                <p class="muted">
                    {tr!("vault.migrate_lead")}
                </p>
                <button
                    type="button"
                    class="btn primary"
                    prop:disabled=move || busy.get() || !unlocked.get()
                    on:click=move |_| {
                        if !unlocked.get() {
                            err.set(Some(crate::i18n::t("vault.unlock_before_migrating").into()));
                            return;
                        }
                        busy.set(true);
                        err.set(None);
                        msg.set(Some(crate::i18n::t("vault.migrating").into()));
                        let sess = use_vault_session();
                        leptos::task::spawn_local(async move {
                            match migrate_all_owned(&sess).await {
                                Ok(m) => match vault_activate().await {
                                    Ok(s) => {
                                        status.set(Some(s));
                                        msg.set(Some(crate::i18n::tf("vault.activated", &[("summary", &m)])));
                                    }
                                    Err(e) => err.set(Some(e.to_string())),
                                },
                                Err(e) => err.set(Some(e)),
                            }
                            busy.set(false);
                        });
                    }
                >
                    {tr!("vault.migrate")}
                </button>
            </Show>

            <Show when=move || status.get().map(|s| s.vault_status != "disabled").unwrap_or(false)>
                <div style="margin-top:0.75rem">
                    <Show when=move || !unlocked.get()>
                        <label>
                            {tr!("vault.unlock_with_key")}
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
                                        msg.set(Some(crate::i18n::t("vault.unlocked_msg").into()));
                                    }
                                    Err(e) => err.set(Some(e)),
                                }
                            }
                        >
                            {tr!("vault.unlock")}
                        </button>
                    </Show>
                    <Show when=move || unlocked.get()>
                        <p class="ok">{tr!("vault.unlocked_here")}</p>
                        <button
                            type="button"
                            class="btn ghost"
                            on:click=move |_| {
                                use_vault_session().lock();
                                msg.set(Some(crate::i18n::t("vault.locked_msg").into()));
                            }
                        >
                            {tr!("vault.lock")}
                        </button>
                    </Show>
                </div>
            </Show>
        </div>
    }
}

/// Server vault state (`disabled` / `migrating` / `active`) as a label.
fn vault_status_label(raw: &str) -> String {
    let key = match raw {
        "disabled" => "vault.state_disabled",
        "migrating" => "vault.state_migrating",
        "active" => "vault.state_active",
        _ => return raw.to_string(),
    };
    crate::i18n::t(key).to_string()
}

/// Unlock gate for vault-sealed list/detail pages.
#[component]
pub fn VaultUnlockGate(
    /// i18n key of the explanation shown above the recovery-key field.
    message: &'static str,
) -> impl IntoView {
    let recovery = RwSignal::new(String::new());
    let error = RwSignal::new(Option::<String>::None);
    let unlocked = use_vault_session().unlocked();

    view! {
        <div class="card" style="max-width: 32rem; margin: 2rem auto;">
            <h2>{tr!("vault.gate_title")}</h2>
            <p class="muted">{move || crate::i18n::t(message)}</p>
            <label>
                {tr!("vault.recovery_key")}
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
                    if let Err(e) = use_vault_session().unlock_from_recovery(&recovery.get()) {
                        error.set(Some(e));
                    }
                }
            >
                {tr!("vault.unlock")}
            </button>
            <Show when=move || unlocked.get()>
                <p class="ok">{tr!("vault.gate_unlocked")}</p>
            </Show>
        </div>
    }
}
