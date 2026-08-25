# Color

## Principle

This is a terminal, not a billboard. `data-viz-dense` bans decoration color outright — there
is no brand hue anywhere in this system. `color-accent` is the maximum-contrast neutral for
the current mode (near-white ink on dark, near-black ink on light), used for primary actions,
focus rings, and active states. The only hues that exist are semantic: success, warning,
danger, info, and a six-step categorical sequence reserved for charts. If a future brief wants
a brand accent, that is a new decision requiring sign-off — it is not something this system
grows into by accident.

Neutral base is a single true-neutral (zinc) family, used identically in concept across both
modes — never mixed with a warm gray. Elevation in both modes is expressed by *surface steps*
(see `04-surfaces-elevation.md`), not by shadow.

## Tokens

| Token | Dark | Light | Use |
|---|---|---|---|
| `color-bg-canvas` | `#0a0a0a` | `#f4f4f5` | Page background |
| `color-bg-surface` | `#171717` | `#ffffff` | Cards, panels, table rows |
| `color-bg-surface-raised` | `#1f1f1f` | `#ffffff` | Hovered/focused row, nested panel |
| `color-bg-surface-overlay` | `#262626` | `#ffffff` | Modal, popover, dropdown body |
| `color-fg-ink` | `#f5f5f4` | `#18181b` | Headings, primary numerals |
| `color-fg-body` | `#d4d4d3` | `#3f3f46` | Body copy, table cell text |
| `color-fg-muted` | `#888884` | `#5f5f66` | Helper text, timestamps, disabled |
| `color-fg-on-accent` | `#0a0a0a` | `#ffffff` | Text sitting on `color-accent` fill |
| `color-border-default` | `rgba(245,245,244,.08)` | `rgba(24,24,27,.08)` | Row/column hairlines |
| `color-border-strong` | `rgba(245,245,244,.16)` | `rgba(24,24,27,.16)` | Table header rule, input focus outline base |
| `color-accent` | `#f5f5f4` | `#18181b` | Primary button/link/focus |
| `color-accent-hover` / `-active` | `#e5e5e4` / `#d4d4d3` | `#3f3f46` / `#52525b` | Interaction states |
| `color-success` / `-subtle` | `#34d058` / 14% tint | `#1a7f37` / 10% tint | Up, healthy, complete |
| `color-warning` / `-subtle` | `#f1a72b` / 14% tint | `#8a5b00` / 10% tint | Degraded, needs attention |
| `color-danger` / `-subtle` | `#f85149` / 14% tint | `#cf222e` / 10% tint | Down, fault, destructive |
| `color-info` / `-subtle` | `#58a6ff` / 14% tint | `#0969da` / 10% tint | Neutral status, informational |
| `color-overlay-scrim` | `rgba(10,10,10,.72)` | `rgba(24,24,27,.48)` | Modal/sheet backdrop |

Every text/background pairing above is verified independently for both modes — see
`07-accessibility.md` for the full contrast table.

## Usage

```css
.metric-value { color: var(--color-fg-ink); font-family: var(--font-family-mono); }
.metric-delta--up { color: var(--color-success); }
.badge--warning { background: var(--color-warning-subtle); color: var(--color-warning); }
```

```css
/* Elevated row on hover — a lightness step, never a shadow */
.data-row:hover { background: var(--color-bg-surface-raised); }
```

Chart/map colors live in `11-data-viz.md` — they are a separate token family
(`color-chart-series-*`, `color-map-*`) so a "series 2 is amber" never gets confused with
"warning is amber"; both happen to use the same hue by design, but a categorical series
assignment is not a state claim.

## Common mistakes

- Reaching for `color-accent` as a "brand color" on marketing-flavored surfaces (an onboarding
  screen, an empty state illustration) — it isn't one. It is functionally "ink." If a surface
  wants warmth, that's an imagery or copy decision, not a color one.
- Using `color-success` / `color-danger` for a categorical chart series (e.g., "Car A" vs
  "Car B" in a comparison chart) — that implies one car is good and one is bad. Use
  `color-chart-series-*` for anything that isn't literally a state.
- Applying dark-mode hex values at reduced opacity to fake a light-mode tint (`#34d058` at 10%
  on white reads sickly-pale, not "success"). Light-mode subtle tints derive from the
  light-mode *text* hue (`#1a7f37`), not the dark-mode one.
