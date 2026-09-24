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
use crate::vault::{load_car_dek, use_vault_session, wrap_and_upload_dek};

fn confirm(msg: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.confirm_with_message(msg).ok())
        .unwrap_or(false)
}

fn date_only(iso: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(iso.trim())
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_else(|_| iso.split('T').next().unwrap_or(iso).to_string())
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
                        format!("Invitation sent to {to}.")
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
        if !confirm(&format!(
            "Leave “{name}”? You lose access to its trips until the owner invites you again."
        )) {
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
                    "Sharing"
                </h2>
                <Show when=move || car.with(|c| c.is_some()) && !is_owner()>
                    <button
                        type="button"
                        class="btn ghost btn-sm err"
                        prop:disabled=move || busy.get()
                        on:click=leave
                    >
                        <Icon name="sign-out" size=IconSize::Sm />
                        "Leave this car"
                    </button>
                </Show>
            </div>

            <Show when=move || is_owner() && !sealed()>
                <div class="live-sharing-row">
                    <div>
                        <div class="live-sharing-title">
                            <Icon name="broadcast" size=IconSize::Sm color=IconColor::Accent />
                            "Live position"
                        </div>
                        <div class="muted field-hint">
                            {move || match car.with(|c| c.as_ref().and_then(|c| c.share_live_position)) {
                                Some(true) => "People this car is shared with can see where it is now.",
                                Some(false) => "Only you see where this car is now.",
                                None => "Choose whether people this car is shared with can see where it is now.",
                            }}
                        </div>
                    </div>
                    <div class="seg-control" role="group" aria-label="Share live position">
                        {[(true, "Shared"), (false, "Private")]
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
                                        {label}
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
                        aria-label="Email to invite"
                        prop:value=move || email.get()
                        on:input=move |ev| email.set(event_target_value(&ev))/>
                    <select style="max-width:140px" aria-label="Role" prop:value=move || role.get()
                        on:change=move |ev| role.set(event_target_value(&ev))>
                        <option value="viewer">"viewer"</option>
                        <option value="editor">"editor"</option>
                    </select>
                    <button class="btn" prop:disabled=move || busy.get() || email.get().trim().is_empty() on:click=invite>
                        <Icon name="user-plus" />
                        "Invite"
                    </button>
                </div>
                <p class="field-hint">
                    {move || if sealed() {
                        "They get access once they accept. For this vault car, share the vault key with them afterwards."
                    } else {
                        "They get access once they accept the invitation."
                    }}
                </p>
            </Show>
            <Show when=move || notice.get().is_some()>
                <div class="success" role="status">{move || notice.get().unwrap_or_default()}</div>
            </Show>

            <Show when=move || is_owner() && !invites.get().is_empty()>
                <h3 class="garage-subtitle">"Pending invitations"</h3>
                <table class="table">
                    <thead><tr><th>"Email"</th><th>"Role"</th><th>"Sent"</th><th></th></tr></thead>
                    <tbody>
                        <For
                            each=move || invites.get()
                            key=|i| i.id.clone()
                            children=move |i| {
                                let invite_id = i.id.clone();
                                let who = i.email.clone();
                                view! {
                                    <tr>
                                        <td data-label="Email">{i.email.clone()}</td>
                                        <td data-label="Role"><span class=format!("badge {}", i.role)>{i.role.clone()}</span></td>
                                        <td class="num" data-label="Sent">{date_only(&i.created_at)}</td>
                                        <td data-label="">
                                            <button
                                                type="button"
                                                class="btn ghost btn-sm err"
                                                prop:disabled=move || busy.get()
                                                on:click=move |_| {
                                                    if !confirm(&format!("Cancel the invitation to {who}?")) {
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
                                                "Cancel"
                                            </button>
                                        </td>
                                    </tr>
                                }
                            }
                        />
                    </tbody>
                </table>
            </Show>

            <h3 class="garage-subtitle">"Members"</h3>
            <Show
                when=move || !shares.get().is_empty()
                fallback=move || view! {
                    <p class="muted">{move || if is_owner() { "Not shared with anyone yet." } else { "—" }}</p>
                }
            >
                <table class="table">
                    <thead>
                        <tr>
                            <th>"User"</th>
                            <th>"Email"</th>
                            <th>"Role"</th>
                            <Show when=move || is_owner() && sealed()>
                                <th>"Vault key"</th>
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
                                                let _ = notice.try_set(Some("Vault key shared.".into()));
                                            }
                                            Err(e) => {
                                                let _ = error.try_set(Some(format!("Could not share the vault key: {e}")));
                                            }
                                        }
                                        let _ = busy.try_set(false);
                                    });
                                };
                                view! {
                                    <tr>
                                        <td data-label="User">{s.name.clone()}</td>
                                        <td data-label="Email">{s.email.clone()}</td>
                                        <td data-label="Role">
                                            <span class=format!("badge {}", s.role)>
                                                <span class="icon-label">
                                                    <Icon name=role_icon size=IconSize::Sm />
                                                    {s.role.clone()}
                                                </span>
                                            </span>
                                        </td>
                                        <Show when=move || is_owner() && sealed()>
                                            <td data-label="Vault key">
                                                {
                                                    let has_key = has_key.clone();
                                                    let share_key = share_key.clone();
                                                    move || {
                                                        if has_key() {
                                                            view! { <span class="pill pill-ok">"Shared"</span> }.into_any()
                                                        } else if !has_pubkey {
                                                            view! { <span class="muted">"No vault identity yet"</span> }.into_any()
                                                        } else {
                                                            let share_key = share_key.clone();
                                                            view! {
                                                                <button
                                                                    type="button"
                                                                    class="btn secondary btn-sm"
                                                                    title=move || if unlocked.get() { "" } else { "Unlock the vault first" }
                                                                    prop:disabled=move || busy.get() || !unlocked.get()
                                                                    on:click=share_key
                                                                >
                                                                    <Icon name="key" size=IconSize::Sm />
                                                                    "Share vault key"
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
                        let by = i
                            .invited_by
                            .clone()
                            .filter(|s| !s.is_empty())
                            .map(|s| format!(" · from {s}"))
                            .unwrap_or_default();
                        let is_busy = move || busy.get().as_deref() == Some(id_busy.as_str());
                        let is_busy2 = is_busy.clone();
                        view! {
                            <li class="invite-item">
                                <div>
                                    <div class="invite-car">
                                        <Icon name="car" size=IconSize::Sm color=IconColor::Device />
                                        {if i.car_name.is_empty() { "A car".to_string() } else { i.car_name.clone() }}
                                    </div>
                                    <div class="muted invite-meta">
                                        {format!("as {}{by} · {}", i.role, date_only(&i.created_at))}
                                    </div>
                                </div>
                                <div class="row">
                                    <button type="button" class="btn primary btn-sm"
                                        prop:disabled=is_busy
                                        on:click=move |_| respond(ia.clone(), true)>
                                        "Accept"
                                    </button>
                                    <button type="button" class="btn ghost btn-sm"
                                        prop:disabled=is_busy2
                                        on:click=move |_| respond(idecline.clone(), false)>
                                        "Decline"
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
                    {format!("You now have access to {}. ", if name.is_empty() { "the car".to_string() } else { name })}
                    <A href=format!("/app/cars/{id}")>"Open it"</A>
                </div>
            }
        })
    };

    if compact {
        view! {
            {accepted_note}
            <Show when=move || !invites.get().is_empty()>
                <section class="card invite-banner" aria-label="Car invitations">
                    <h2 class="section-title">
                        <Icon name="envelope-simple" color=IconColor::Accent />
                        {move || {
                            let n = invites.get().len();
                            format!("{n} car invitation{} waiting", if n == 1 { "" } else { "s" })
                        }}
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
                    "Invitations"
                </h2>
                <p class="muted">"Cars other people invited you to. Accepting gives you access with the role they chose."</p>
                {accepted_note}
                <Show
                    when=move || !invites.get().is_empty()
                    fallback=move || view! {
                        <p class="muted">{move || if loaded.get() { "No pending invitations." } else { "Loading…" }}</p>
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
