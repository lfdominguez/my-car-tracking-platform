//! Trip export menu (#117).
//!
//! Files download straight from `GET /api/trips/{id}/export`.

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::api::Trip;
use crate::components::{Icon, IconSize};
use crate::i18n::t;

/// `(format, i18n key of its menu label)`.
const FORMATS: [(&str, &str); 4] = [
    ("gpx", "export.gpx"),
    ("kml", "export.kml"),
    ("geojson", "export.geojson"),
    ("csv", "export.csv"),
];

/// Close the `<details>` menu a click came from.
fn close_menu(ev: &web_sys::MouseEvent) {
    let details = ev
        .target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        .and_then(|el| el.closest("details").ok().flatten());
    if let Some(d) = details {
        let _ = d.remove_attribute("open");
    }
}

/// Print the trip page (the browser offers "Save as PDF"). Charts paint to
/// canvas in the app theme, so a dark UI is switched to light for the print and
/// back afterwards; `window.print()` blocks until the dialog closes.
fn print_page(theme: crate::components::theme::ThemeSignal) {
    let dark = theme.theme.get_untracked() == crate::components::theme::Theme::Dark;
    if dark {
        theme.toggle();
    }
    leptos::task::spawn_local(async move {
        // Let the charts repaint in the light palette first.
        gloo_timers::future::TimeoutFuture::new(if dark { 700 } else { 50 }).await;
        if let Some(w) = web_sys::window() {
            let _ = w.print();
        }
        if dark {
            theme.toggle();
        }
    });
}

/// "Export" dropdown for the trip page.
#[component]
pub fn TripExportMenu(trip: RwSignal<Option<Trip>>) -> impl IntoView {
    let theme = crate::components::use_theme();
    view! {
        <details class="export-menu">
            <summary class="btn secondary sm">
                <span class="icon-label">
                    <Icon name="download-simple" size=IconSize::Sm />
                    {tr!("export.export")}
                </span>
            </summary>
            <div class="export-menu-list" role="menu">
                {FORMATS
                    .into_iter()
                    .map(|(format, label)| {
                        let href = move || {
                            trip.with(|t| {
                                t.as_ref()
                                    .map(|t| format!("/api/trips/{}/export?format={format}", t.id))
                                    .unwrap_or_default()
                            })
                        };
                        view! {
                            <a class="export-menu-item" role="menuitem" href=href download="" rel="nofollow">
                                {move || t(label)}
                            </a>
                        }
                    })
                    .collect_view()}
                <button
                    type="button"
                    class="export-menu-item export-menu-print"
                    role="menuitem"
                    on:click=move |ev| {
                        close_menu(&ev);
                        print_page(theme);
                    }
                >
                    {tr!("export.print")}
                </button>
            </div>
        </details>
    }
}
