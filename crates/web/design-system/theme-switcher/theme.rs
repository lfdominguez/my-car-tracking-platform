//! Theme switcher — CSS-variable swap via `data-theme` on `<html>`.
//!
//! Mechanism: tokens/tokens.css scopes every color custom property under
//! `[data-theme="dark"]` and `[data-theme="light"]`. This module is the only place that
//! writes the attribute; every component just reads `var(--color-*)` and never branches on
//! theme in Rust/Leptos logic.
//!
//! Drop this file in `crates/web/src/theme.rs` (or `src/components/theme.rs`, matching the
//! existing `src/components/` convention) and call `Theme::init()` once near app root, then
//! `use_theme()` from anywhere that needs to read/toggle it (e.g. a settings row, or a topbar
//! toggle button per `components/navbar.contract.md`).

use leptos::prelude::*;

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

/// Reads the attribute the inline boot script (theme-switcher/inline-boot.html) already set
/// on `<html>` before the WASM app hydrated — this must never guess independently, or it can
/// disagree with what the user actually sees for one frame.
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

/// Global reactive theme signal. Call once near the app root (e.g. in `App()` before
/// rendering the shell) — subsequent calls elsewhere in the tree should use `use_theme()`
/// to read the existing context instead of re-initializing.
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

    pub fn set(&self, theme: Theme) {
        write_attr(theme);
        self.set_theme.set(theme);
    }
}

/// Read/toggle the theme from any component under `provide_theme()`.
pub fn use_theme() -> ThemeSignal {
    use_context::<ThemeSignal>().expect("provide_theme() must run above this component in the tree")
}

/// Example toggle control — an icon-only button per `components/button.contract.md`
/// (`icon-only`, `ghost` variant), swapping the Phosphor glyph by theme.
#[component]
pub fn ThemeToggle() -> impl IntoView {
    let theme = use_theme();

    view! {
        <button
            class="btn btn--ghost btn--icon-only"
            aria-label="Toggle color theme"
            on:click=move |_| theme.toggle()
        >
            <i
                class=move || if theme.theme.get() == Theme::Dark { "ph ph-sun" } else { "ph ph-moon" }
                aria-hidden="true"
            ></i>
        </button>
    }
}
