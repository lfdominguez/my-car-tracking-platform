# Color

## Principle

This is an instrument cluster, not a billboard. Color earns its place by encoding
*state* or by marking the *one* action that matters on a screen — never by decorating.

The system has exactly one brand hue: an electric blue, paired with a cyan for the
signature gradient. `color-accent` **is** that blue in both modes (v1 used a
maximum-contrast neutral instead, which made every primary button read as a black or
white slab and left the product with no identity at a glance). Everything else is
either neutral or semantic: success, warning, danger, and a six-step categorical
sequence reserved for charts.

The neutral ramp is deliberately **cool** — a blue-tinted near-black in dark mode,
a blue-tinted off-white in light mode — so brand blue sits inside the same family as
the chrome instead of floating on top of a true gray. Both modes are authored
independently; neither is an inversion of the other.

## Tokens

| Token | Dark | Light | Use |
|---|---|---|---|
| `color-bg-canvas` | `#08090d` | `#f5f6fa` | Page background |
| `color-bg-surface` | `#0f1117` | `#ffffff` | Cards, panels, table rows |
| `color-bg-surface-raised` | `#161922` | `#ffffff` | Hovered/focused row, nested panel |
| `color-bg-surface-overlay` | `#1d212c` | `#ffffff` | Modal, popover, dropdown body |
| `color-bg-inset` | `#0b0d12` | `#eef0f6` | Inputs, metric wells, control tracks |
| `color-fg-ink` | `#f1f4f9` | `#0d1220` | Headings, primary numerals |
| `color-fg-body` | `#c2c9d6` | `#3d4759` | Body copy, table cell text |
| `color-fg-muted` | `#838da3` | `#646e83` | Helper text, micro-labels, timestamps |
| `color-fg-faint` | `#5d6579` | `#8b93a5` | Placeholders, group labels, disabled |
| `color-fg-on-accent` | `#04070f` | `#ffffff` | Text sitting on `color-accent` fill |
| `color-border-default` | `rgba(160,178,214,.10)` | `rgba(13,18,32,.09)` | Row/column hairlines |
| `color-border-strong` | `rgba(160,178,214,.20)` | `rgba(13,18,32,.18)` | Header rules, hover borders |
| `color-border-accent` | `rgba(90,154,255,.42)` | `rgba(37,99,235,.40)` | Focused/selected surface edge |
| `color-accent` | `#5a9aff` | `#2563eb` | Primary button, link, active nav |
| `color-accent-hover` / `-active` | `#79b0ff` / `#3f83ee` | `#1d4ed8` / `#1e40af` | Interaction states |
| `color-accent-soft` / `-softer` | 14% / 7% tint | 10% / 5% tint | Selected fill, hover wash |
| `color-accent-2` | `#37d9e8` | `#0891b2` | Gradient partner, device/telemetry icons |
| `color-success` / `-subtle` | `#3ddc84` / 14% | `#148f4b` / 10% | Healthy, complete, tank ≥ 50% |
| `color-warning` / `-subtle` | `#ffb545` / 14% | `#a35a00` / 10% | Degraded, tank 20–50% |
| `color-danger` / `-subtle` | `#ff6b63` / 14% | `#d02a35` / 10% | Fault, destructive, tank < 20% |
| `color-info` / `-subtle` | = accent | = accent | Neutral status (aliases the brand hue) |
| `color-overlay-scrim` | `rgba(4,6,12,.72)` | `rgba(13,18,32,.44)` | Modal/sheet backdrop |

Every text/background pairing above is verified independently for both modes — see
`07-accessibility.md`.

## Usage

```css
.btn.primary { background-color: var(--color-accent); color: var(--color-fg-on-accent); }
.nav a.active { background: var(--color-accent-soft); }
.nav a.active::before { background: var(--gradient-brand-vivid); }
.card { background: var(--gradient-surface), var(--color-bg-surface); }
```

## Common mistakes

- **Using the brand blue to mean "healthy."** Blue is identity and interaction. State is
  success/warning/danger. A blue tank gauge tells the driver nothing.
- **Reaching for a second brand hue.** The violet and rose in the chart ramp are
  *categorical series colors*, not accents; using them for a button invents a second brand.
- **Coloring a whole card in `accent-soft` to draw attention.** That tint marks *selection*.
  If everything is selected, nothing is.
- **Hardcoding a hex** because "it's just this one chevron." Every hue in the product traces
  back to a token; ECharts, which can't read CSS variables, reads them through `chart_theme()`
  in `components/charts.rs` instead of hardcoding a parallel palette.
