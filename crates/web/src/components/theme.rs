//! Theme switcher — CSS-variable swap via `data-theme` on `<html>`.
//!
//! Mechanism: design-system/tokens/tokens.css scopes every color custom property
//! under `[data-theme="dark"]` and `[data-theme="light"]`. This module is the only
//! place that writes the attribute; every other selector in style.css just reads
//! `var(--bg)`/`var(--text)`/etc. and never branches on theme.
//!
//! The inline boot script in index.html already sets `data-theme` on `<html>`
//! before this module hydrates (avoids a flash of the wrong theme), so
//! `provide_theme()` reads that existing attribute rather than guessing again.

use leptos::prelude::*;

use crate::components::Icon;

const STORAGE_KEY: &str = "theme";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Dark,
    Light,
}

impl Theme {
    fn as_str(self) -> &'static str {
        match self {
            Theme::Dark => "dark",
            Theme::Light => "light",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "dark" => Some(Theme::Dark),
            "light" => Some(Theme::Light),
            _ => None,
        }
    }

    fn toggle(self) -> Self {
        match self {
            Theme::Dark => Theme::Light,
            Theme::Light => Theme::Dark,
        }
    }
}

fn read_current_attr() -> Theme {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
        .and_then(|el| el.get_attribute("data-theme"))
        .and_then(|s| Theme::from_str(&s))
        .unwrap_or(Theme::Dark)
}

fn write_attr(theme: Theme) {
    let Some(win) = web_sys::window() else { return };
    let Some(doc) = win.document() else { return };
    let Some(el) = doc.document_element() else { return };
    let _ = el.set_attribute("data-theme", theme.as_str());

    if let Ok(Some(storage)) = win.local_storage() {
        let _ = storage.set_item(STORAGE_KEY, theme.as_str());
    }
}

/// Global reactive theme signal. Call once near the app root (main.rs, alongside
/// `provide_vault_session()`); read/toggle it elsewhere via `use_theme()`.
pub fn provide_theme() {
    let (theme, set_theme) = signal(read_current_attr());
    provide_context(ThemeSignal { theme, set_theme });
}

#[derive(Clone, Copy)]
pub struct ThemeSignal {
    pub theme: ReadSignal<Theme>,
    set_theme: WriteSignal<Theme>,
}

impl ThemeSignal {
    pub fn toggle(&self) {
        let next = self.theme.get_untracked().toggle();
        write_attr(next);
        self.set_theme.set(next);
    }
}

/// Read/toggle the theme from any component under `provide_theme()`.
pub fn use_theme() -> ThemeSignal {
    use_context::<ThemeSignal>().expect("provide_theme() must run above this component in the tree")
}

#[component]
pub fn ThemeToggle() -> impl IntoView {
    let theme = use_theme();

    view! {
        <button
            type="button"
            class="btn icon-btn"
            aria-label=move || if theme.theme.get() == Theme::Dark { "Switch to light theme" } else { "Switch to dark theme" }
            on:click=move |_| theme.toggle()
        >
            {move || if theme.theme.get() == Theme::Dark {
                view! { <Icon name="sun" /> }.into_any()
            } else {
                view! { <Icon name="moon" /> }.into_any()
            }}
        </button>
    }
}
