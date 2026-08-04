use leptos::prelude::*;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum IconSize {
    Sm,
    #[default]
    Md,
    Lg,
    Xl,
}

impl IconSize {
    fn class(self) -> &'static str {
        match self {
            Self::Sm => "sm",
            Self::Md => "md",
            Self::Lg => "lg",
            Self::Xl => "xl",
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum IconColor {
    #[default]
    Default,
    Accent,
    Success,
    Warn,
    /// Reserved for destructive actions (revoke/delete).
    #[allow(dead_code)]
    Danger,
    Device,
}

impl IconColor {
    fn class(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::Accent => Some("accent"),
            // Avoid bare "success" — that class is used by flash/banner boxes in style.css.
            Self::Success => Some("tone-success"),
            Self::Warn => Some("warn"),
            Self::Danger => Some("danger"),
            Self::Device => Some("device"),
        }
    }
}

/// Phosphor icon name without the `ph-` prefix (e.g. `"car"`, `"gas-pump"`).
#[component]
pub fn Icon(
    /// Phosphor glyph name, e.g. `"car"`
    #[prop(into)]
    name: String,
    #[prop(optional)] size: IconSize,
    #[prop(optional)] color: IconColor,
    /// If set, icon is meaningful alone; otherwise treat as decorative.
    #[prop(optional, into)]
    aria_label: Option<String>,
) -> impl IntoView {
    let mut class = format!("icon {} ph-duotone ph-{name}", size.class());
    if let Some(c) = color.class() {
        class.push(' ');
        class.push_str(c);
    }
    let decorative = aria_label.is_none();
    view! {
        <i
            class=class
            aria-hidden=decorative.then_some("true")
            aria-label=aria_label
        ></i>
    }
}
