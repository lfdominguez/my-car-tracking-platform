# Iconography & imagery

## Deviation flagged: Phosphor, not Material Symbols

`/ux-system`'s default icon set is Material Symbols. This system deliberately ships **Phosphor**
instead, because `crates/web` already vendors it self-hosted at
`public/vendor/{regular,duotone}/Phosphor*.{woff,woff2,ttf}`, wired into `index.html` via
`<link rel="stylesheet" href="/vendor/phosphor-{regular,duotone}.css">`, specifically to satisfy
a CSP `script-src 'self'` constraint noted inline in that file. Swapping to Material Symbols
would mean vendoring a new (larger) variable-font file, re-pointing the CSP-safe asset pipeline,
and replacing every existing glyph reference in the app for no aesthetic gain — Phosphor's
regular weight is a clean, consistent single-family outline set that fits `data-viz-dense`'s
"one icon family, one stroke weight" requirement just as well. **This is open to reconsideration**
if there's a reason Material Symbols specifically matters (e.g. a future requirement for its
variable FILL/GRAD/wght/opsz axes) — flagging it here so it isn't silently locked in.

## Principle

One family (Phosphor), one weight tier as the default (**Regular**), used consistently at
`16px`/`20px`/`24px` sizes matched to the type scale they sit beside (`text-sm`→16px icon,
`text-lg`→20px icon, `text-2xl`+→24px icon). **Duotone** is reserved for exactly one job:
marking the active/selected state (active nav item, selected car) — never mixed with Regular at
the same hierarchy level, and never used as decoration.

```css
.icon { font-family: 'Phosphor'; font-size: var(--text-lg); line-height: 1; color: currentColor; }
.icon--active { font-family: 'Phosphor-Duotone'; color: var(--color-accent); }
```

No `font-variation-settings` axis exists for Phosphor (that API is Material-Symbols-specific);
weight/fill differentiation here is handled by swapping the font-family between Regular and
Duotone, not by animating a variable axis.

## Tokens

| Token | Value | Use |
|---|---|---|
| icon size — small | `text-sm` (13px icon slot → render at 16px) | Inline with dense table/list text |
| icon size — medium | `text-lg` (19px icon slot → render at 20px) | Buttons, nav items, card headers |
| icon size — large | `text-2xl`+ (28px icon slot → render at 24px) | Empty states, page headers |

Icon color always inherits `currentColor` — never a hardcoded hex — so an icon inside a danger
badge is automatically `color-danger`, inside muted helper text is automatically `fg-muted`.

## Imagery rules

This is a utility dashboard, not a marketing surface — imagery is minimal and functional, never
decorative stock:

- **Car photos** (user-uploaded or VIN-decoded stock photos): 4:3 or 1:1, `radius-avatar` only
  when used as a small identifier chip (nav, list row); full `radius-surface` (0, no rounding)
  when shown as a larger detail-view image.
  - **If no photo exists, omit the image entirely** — don't substitute a generic car silhouette
    icon at photo scale; a Phosphor `car` glyph at icon scale in a neutral chip is the correct
    fallback, not a fake-photo placeholder.
- **Map imagery** is the primary "photography" of this product — see `11-data-viz.md`.
- No stock lifestyle photography, no isometric illustration, no icon-as-hero-art. If a section
  needs visual weight beyond text, it's a real map, a real chart, or a real vehicle photo.

## Common mistakes

- Mixing Duotone into a list of otherwise-Regular icons to "add visual interest" — Duotone is a
  state signal (active/selected), not a decoration tier.
- Hardcoding an icon's color instead of `currentColor` — breaks automatically when the icon sits
  inside a themed badge or dark/light mode switch.
- Using the same repeated icon (e.g. a generic "car" glyph) across every differentiated list
  item as a stand-in for a real car photo — if there's no photo, drop to a neutral text/initial
  chip instead of a repeated icon that implies "these are the same."
