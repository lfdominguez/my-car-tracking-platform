//! Car sharing by invitation.
//!
//! An owner invites an email; the share exists only once that person accepts.
//! The car's Sharing card lists members and pending invites (owner), lets a
//! sharee leave, and — for vault cars — lets the owner hand the car key to
//! members who published a vault identity. [`PendingInvites`] is the invitee's
//! side, shown on the dashboard and in Settings.

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_navigate;

use crate::api::{
    Car, CarInvite, MyInvite, Share, accept_share_invite, cancel_car_invite, create_share,
    decline_share_invite, leave_shared_car, list_car_invites, list_shares, my_share_invites,
    set_live_sharing, vault_list_deks,
};
use crate::components::{Icon, IconColor, IconSize};
use crate::i18n::{role_label, t, tf, tp};
use crate::vault::{load_car_dek, use_vault_session, wrap_and_upload_dek};

fn confirm(msg: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.confirm_with_message(msg).ok())
        .unwrap_or(false)
}

fn date_only(iso: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(iso.trim())
        .map(|d| {
            crate::i18n::date(
                &d.with_timezone(&chrono::Local).date_naive(),
                crate::i18n::date_pattern(),
            )
        })
        .unwrap_or_else(|_| crate::i18n::iso_date(iso.split('T').next().unwrap_or(iso)))
}

/// The car page's Sharing card.
#[component]
pub fn SharingCard(
    car: RwSignal<Option<Car>>,
    #[prop(into)] car_id: Signal<String>,
    /// Page-level error banner.
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let navigate = StoredValue::new(use_navigate());
    let vault = use_vault_session();
    let unlocked = vault.unlocked();
    let shares = RwSignal::new(Vec::<Share>::new());
    let invites = RwSignal::new(Vec::<CarInvite>::new());
    // Members that already hold a wrapped copy of the vault car key.
    let wrapped_for = RwSignal::new(Vec::<String>::new());
    let email = RwSignal::new(String::new());
    let role = RwSignal::new("viewer".to_string());
    let notice = RwSignal::new(Option::<String>::None);
    let busy = RwSignal::new(false);
    let live_busy = RwSignal::new(false);
    let refresh = RwSignal::new(0u32);

    let is_owner = move || car.with(|c| c.as_ref().is_some_and(|c| c.role == "owner"));
    let sealed = move || car.with(|c| c.as_ref().is_some_and(|c| c.vault_sealed));

    Effect::new(move |_| {
        let id = car_id.get();
        refresh.track();
        let owner = is_owner();
        let is_sealed = sealed();
        if id.is_empty() || car.with(|c| c.is_none()) {
            return;
        }
        leptos::task::spawn_local(async move {
            match list_shares(&id).await {
                Ok(s) => {
                    let _ = shares.try_set(s);
                }
                Err(e) => {
                    let _ = error.try_set(Some(e.to_string()));
                }
            }
            if owner {
                if let Ok(i) = list_car_invites(&id).await {
                    let _ = invites.try_set(i);
                }
                if is_sealed && let Ok(deks) = vault_list_deks(&id).await {
                    let ids = deks
                        .iter()
                        .filter_map(|d| d.get("recipient_user_id")?.as_str().map(str::to_string))
                        .collect();
                    let _ = wrapped_for.try_set(ids);
                }
            }
        });
    });

    let invite = move |_| {
        let id = car_id.get_untracked();
        let to = email.get_untracked().trim().to_string();
        if to.is_empty() {
            return;
        }
        let r = role.get_untracked();
        busy.set(true);
        notice.set(None);
        leptos::task::spawn_local(async move {
            match create_share(&id, &to, &r).await {
                Ok(resp) => {
                    let _ = email.try_set(String::new());
                    let msg = if resp.message.trim().is_empty() {
                        tf("share.invitation_sent", &[("email", &to)])
                    } else {
                        resp.message
                    };
                    let _ = notice.try_set(Some(msg));
                    refresh.update(|n| *n = n.wrapping_add(1));
                }
                Err(e) => {
                    let _ = error.try_set(Some(e.to_string()));
                }
            }
            let _ = busy.try_set(false);
        });
    };

    let leave = move |_| {
        let name = car.with_untracked(|c| c.as_ref().map(|c| c.name.clone()).unwrap_or_default());
        if !confirm(&tf("share.confirm_leave", &[("name", &name)])) {
            return;
        }
        let id = car_id.get_untracked();
        busy.set(true);
        leptos::task::spawn_local(async move {
            match leave_shared_car(&id).await {
                Ok(()) => navigate.with_value(|nav| nav("/app/cars", Default::default())),
                Err(e) => {
                    let _ = error.try_set(Some(e.to_string()));
                    let _ = busy.try_set(false);
                }
            }
        });
    };

    view! {
        <div class="card sharing-card">
            <div class="telemetry-section-head">
                <h2 class="section-title">
                    <Icon name="share-network" color=IconColor::Accent />
                    {tr!("share.title")}
                </h2>
                <Show when=move || car.with(|c| c.is_some()) && !is_owner()>
                    <button
                        type="button"
                        class="btn ghost btn-sm err"
                        prop:disabled=move || busy.get()
                        on:click=leave
                    >
                        <Icon name="sign-out" size=IconSize::Sm />
                        {tr!("share.leave")}
                    </button>
                </Show>
            </div>

            <Show when=move || is_owner() && !sealed()>
                <div class="live-sharing-row">
                    <div>
                        <div class="live-sharing-title">
                            <Icon name="broadcast" size=IconSize::Sm color=IconColor::Accent />
                            {tr!("share.live_position")}
                        </div>
                        <div class="muted field-hint">
                            {move || match car.with(|c| c.as_ref().and_then(|c| c.share_live_position)) {
                                Some(true) => t("share.live_on"),
                                Some(false) => t("share.live_off"),
                                None => t("share.live_unset"),
                            }}
                        </div>
                    </div>
                    <div class="seg-control" role="group" aria-label=tr!("share.live_group")>
                        {[(true, "share.shared"), (false, "share.private")]
                            .into_iter()
                            .map(|(value, label)| {
                                let active = move || {
                                    car.with(|c| c.as_ref().and_then(|c| c.share_live_position)) == Some(value)
                                };
                                view! {
                                    <button
                                        type="button"
                                        class=move || if active() { "seg-btn is-active" } else { "seg-btn" }
                                        aria-pressed=move || active().to_string()
                                        prop:disabled=move || live_busy.get()
                                        on:click=move |_| {
                                            let id = car_id.get_untracked();
                                            live_busy.set(true);
                                            leptos::task::spawn_local(async move {
                                                match set_live_sharing(&id, value).await {
                                                    Ok(on) => {
                                                        let _ = car.try_update(|c| {
                                                            if let Some(c) = c.as_mut() {
                                                                c.share_live_position = Some(on);
                                                            }
                                                        });
                                                    }
                                                    Err(e) => {
                                                        let _ = error.try_set(Some(e.to_string()));
                                                    }
                                                }
                                                let _ = live_busy.try_set(false);
                                            });
                                        }
                                    >
                                        {move || t(label)}
                                    </button>
                                }
                            })
                            .collect_view()}
                    </div>
                </div>
            </Show>

            <Show when=is_owner>
                <div class="row">
                    <input style="max-width:260px" type="email" placeholder="user@email.com"
                        aria-label=tr!("share.email_to_invite")
                        prop:value=move || email.get()
                        on:input=move |ev| email.set(event_target_value(&ev))/>
                    <select style="max-width:140px" aria-label=tr!("cars.role") prop:value=move || role.get()
                        on:change=move |ev| role.set(event_target_value(&ev))>
                        <option value="viewer">{tr!("role.viewer")}</option>
                        <option value="editor">{tr!("role.editor")}</option>
                    </select>
                    <button class="btn" prop:disabled=move || busy.get() || email.get().trim().is_empty() on:click=invite>
                        <Icon name="user-plus" />
                        {tr!("share.invite")}
                    </button>
                </div>
                <p class="field-hint">
                    {move || if sealed() {
                        t("share.hint_vault")
                    } else {
                        t("share.hint")
                    }}
                </p>
            </Show>
            <Show when=move || notice.get().is_some()>
                <div class="success" role="status">{move || notice.get().unwrap_or_default()}</div>
            </Show>

            <Show when=move || is_owner() && !invites.get().is_empty()>
                <h3 class="garage-subtitle">{tr!("share.pending")}</h3>
                <table class="table">
                    <thead><tr><th>{tr!("share.email")}</th><th>{tr!("cars.role")}</th><th>{tr!("share.sent")}</th><th></th></tr></thead>
                    <tbody>
                        <For
                            each=move || invites.get()
                            key=|i| i.id.clone()
                            children=move |i| {
                                let invite_id = i.id.clone();
                                let who = i.email.clone();
                                let (role, created) = (i.role.clone(), i.created_at.clone());
                                view! {
                                    <tr>
                                        <td data-label=tr!("share.email")>{i.email.clone()}</td>
                                        <td data-label=tr!("cars.role")><span class=format!("badge {}", i.role)>{move || role_label(&role)}</span></td>
                                        <td class="num" data-label=tr!("share.sent")>{move || date_only(&created)}</td>
                                        <td data-label="">
                                            <button
                                                type="button"
                                                class="btn ghost btn-sm err"
                                                prop:disabled=move || busy.get()
                                                on:click=move |_| {
                                                    if !confirm(&tf("share.confirm_cancel", &[("email", &who)])) {
                                                        return;
                                                    }
                                                    let car = car_id.get_untracked();
                                                    let id = invite_id.clone();
                                                    leptos::task::spawn_local(async move {
                                                        match cancel_car_invite(&car, &id).await {
                                                            Ok(()) => {
                                                                let _ = invites.try_update(|l| l.retain(|x| x.id != id));
                                                            }
                                                            Err(e) => {
                                                                let _ = error.try_set(Some(e.to_string()));
                                                            }
                                                        }
                                                    });
                                                }
                                            >
                                                {tr!("common.cancel")}
                                            </button>
                                        </td>
                                    </tr>
                                }
                            }
                        />
                    </tbody>
                </table>
            </Show>

            <h3 class="garage-subtitle">{tr!("share.members")}</h3>
            <Show
                when=move || !shares.get().is_empty()
                fallback=move || view! {
                    <p class="muted">{move || if is_owner() { t("share.not_shared") } else { "—" }}</p>
                }
            >
                <table class="table">
                    <thead>
                        <tr>
                            <th>{tr!("share.user")}</th>
                            <th>{tr!("share.email")}</th>
                            <th>{tr!("cars.role")}</th>
                            <Show when=move || is_owner() && sealed()>
                                <th>{tr!("share.vault_key")}</th>
                            </Show>
                        </tr>
                    </thead>
                    <tbody>
                        <For
                            each=move || shares.get()
                            key=|s| format!("{}:{}:{}", s.car_id, s.user_id, s.role)
                            children=move |s| {
                                let role_icon = match s.role.as_str() {
                                    "editor" => "pencil-simple",
                                    _ => "eye",
                                };
                                let user_id = s.user_id.clone();
                                let has_key = {
                                    let uid = user_id.clone();
                                    move || wrapped_for.with(|w| w.contains(&uid))
                                };
                                let pubkey = s.vault_identity_pubkey_b64.clone();
                                let has_pubkey = s.vault_has_pubkey && pubkey.is_some();
                                let sess = use_vault_session();
                                let share_key = move |_| {
                                    let Some(pk) = pubkey.clone() else {
                                        return;
                                    };
                                    let car = car_id.get_untracked();
                                    let uid = user_id.clone();
                                    let sess = sess.clone();
                                    busy.set(true);
                                    leptos::task::spawn_local(async move {
                                        let res = match load_car_dek(&sess, &car).await {
                                            Ok(dek) => wrap_and_upload_dek(&sess, &car, &uid, &pk, &dek).await,
                                            Err(e) => Err(e),
                                        };
                                        match res {
                                            Ok(()) => {
                                                let _ = wrapped_for.try_update(|w| w.push(uid));
                                                let _ = notice.try_set(Some(t("share.key_shared").into()));
                                            }
                                            Err(e) => {
                                                let _ = error.try_set(Some(tf("share.key_failed", &[("error", &e)])));
                                            }
                                        }
                                        let _ = busy.try_set(false);
                                    });
                                };
                                let role = s.role.clone();
                                view! {
                                    <tr>
                                        <td data-label=tr!("share.user")>{s.name.clone()}</td>
                                        <td data-label=tr!("share.email")>{s.email.clone()}</td>
                                        <td data-label=tr!("cars.role")>
                                            <span class=format!("badge {}", s.role)>
                                                <span class="icon-label">
                                                    <Icon name=role_icon size=IconSize::Sm />
                                                    {move || role_label(&role)}
                                                </span>
                                            </span>
                                        </td>
                                        <Show when=move || is_owner() && sealed()>
                                            <td data-label=tr!("share.vault_key")>
                                                {
                                                    let has_key = has_key.clone();
                                                    let share_key = share_key.clone();
                                                    move || {
                                                        if has_key() {
                                                            view! { <span class="pill pill-ok">{tr!("share.shared")}</span> }.into_any()
                                                        } else if !has_pubkey {
                                                            view! { <span class="muted">{tr!("share.no_identity")}</span> }.into_any()
                                                        } else {
                                                            let share_key = share_key.clone();
                                                            view! {
                                                                <button
                                                                    type="button"
                                                                    class="btn secondary btn-sm"
                                                                    title=move || if unlocked.get() { "" } else { t("share.unlock_first") }
                                                                    prop:disabled=move || busy.get() || !unlocked.get()
                                                                    on:click=share_key
                                                                >
                                                                    <Icon name="key" size=IconSize::Sm />
                                                                    {tr!("share.share_key")}
                                                                </button>
                                                            }
                                                            .into_any()
                                                        }
                                                    }
                                                }
                                            </td>
                                        </Show>
                                    </tr>
                                }
                            }
                        />
                    </tbody>
                </table>
            </Show>
        </div>
    }
}

/// Invitations waiting for the signed-in user. `compact` renders a banner that
/// disappears when there are none (dashboard); otherwise a Settings card.
#[component]
pub fn PendingInvites(#[prop(optional)] compact: bool) -> impl IntoView {
    let invites = RwSignal::new(Vec::<MyInvite>::new());
    let loaded = RwSignal::new(false);
    let busy = RwSignal::new(Option::<String>::None);
    let error = RwSignal::new(Option::<String>::None);
    let accepted = RwSignal::new(Option::<(String, String)>::None);

    leptos::task::spawn_local(async move {
        match my_share_invites().await {
            Ok(list) => {
                let _ = invites.try_set(list);
            }
            Err(e) => {
                let _ = error.try_set(Some(e.to_string()));
            }
        }
        let _ = loaded.try_set(true);
    });

    let respond = move |invite: MyInvite, accept: bool| {
        busy.set(Some(invite.id.clone()));
        error.set(None);
        leptos::task::spawn_local(async move {
            let res = if accept {
                accept_share_invite(&invite.id).await.map(Some)
            } else {
                decline_share_invite(&invite.id).await.map(|_| None)
            };
            match res {
                Ok(car) => {
                    let _ = invites.try_update(|l| l.retain(|x| x.id != invite.id));
                    if let Some(car_id) = car {
                        let id = if car_id.is_empty() {
                            invite.car_id.clone()
                        } else {
                            car_id
                        };
                        let _ = accepted.try_set(Some((id, invite.car_name.clone())));
                    }
                }
                Err(e) => {
                    let _ = error.try_set(Some(e.to_string()));
                }
            }
            let _ = busy.try_set(None);
        });
    };

    let list = move || {
        view! {
            <ul class="invite-list">
                <For
                    each=move || invites.get()
                    key=|i| i.id.clone()
                    children=move |i| {
                        let (ia, id_busy) = (i.clone(), i.id.clone());
                        let idecline = i.clone();
                        let invited_by = i.invited_by.clone().filter(|s| !s.is_empty());
                        let (role, created, car_name) = (i.role.clone(), i.created_at.clone(), i.car_name.clone());
                        let meta = move || {
                            let by = invited_by
                                .as_ref()
                                .map(|s| tf("share.from", &[("name", s)]))
                                .unwrap_or_default();
                            tf(
                                "share.as_role",
                                &[("role", &role_label(&role)), ("by", &by), ("date", &date_only(&created))],
                            )
                        };
                        let is_busy = move || busy.get().as_deref() == Some(id_busy.as_str());
                        let is_busy2 = is_busy.clone();
                        view! {
                            <li class="invite-item">
                                <div>
                                    <div class="invite-car">
                                        <Icon name="car" size=IconSize::Sm color=IconColor::Device />
                                        {move || if car_name.is_empty() { t("places.a_car").to_string() } else { car_name.clone() }}
                                    </div>
                                    <div class="muted invite-meta">{meta}</div>
                                </div>
                                <div class="row">
                                    <button type="button" class="btn primary btn-sm"
                                        prop:disabled=is_busy
                                        on:click=move |_| respond(ia.clone(), true)>
                                        {tr!("share.accept")}
                                    </button>
                                    <button type="button" class="btn ghost btn-sm"
                                        prop:disabled=is_busy2
                                        on:click=move |_| respond(idecline.clone(), false)>
                                        {tr!("share.decline")}
                                    </button>
                                </div>
                            </li>
                        }
                    }
                />
            </ul>
        }
    };

    let accepted_note = move || {
        accepted.get().map(|(id, name)| {
            view! {
                <div class="success" role="status">
                    {tf("share.now_access", &[("name", &if name.is_empty() { t("share.the_car").to_string() } else { name })])}
                    " "
                    <A href=format!("/app/cars/{id}")>{tr!("share.open_it")}</A>
                </div>
            }
        })
    };

    if compact {
        view! {
            {accepted_note}
            <Show when=move || !invites.get().is_empty()>
                <section class="card invite-banner" aria-label=tr!("share.car_invitations")>
                    <h2 class="section-title">
                        <Icon name="envelope-simple" color=IconColor::Accent />
                        {move || tp("share.waiting", invites.get().len() as i64)}
                    </h2>
                    {list}
                    <Show when=move || error.get().is_some()>
                        <div class="error">{move || error.get().unwrap_or_default()}</div>
                    </Show>
                </section>
            </Show>
        }
        .into_any()
    } else {
        view! {
            <div class="card settings-card" id="invites" style="margin-top:1rem">
                <h2 class="section-title">
                    <Icon name="envelope-simple" color=IconColor::Accent />
                    {tr!("share.invitations")}
                </h2>
                <p class="muted">{tr!("share.invitations_lead")}</p>
                {accepted_note}
                <Show
                    when=move || !invites.get().is_empty()
                    fallback=move || view! {
                        <p class="muted">{move || if loaded.get() { t("share.no_pending") } else { t("common.loading") }}</p>
                    }
                >
                    {list}
                </Show>
                <Show when=move || error.get().is_some()>
                    <div class="error">{move || error.get().unwrap_or_default()}</div>
                </Show>
            </div>
        }
        .into_any()
    }
}
