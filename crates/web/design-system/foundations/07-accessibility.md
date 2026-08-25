# Accessibility floor

## Target

**WCAG 2.1 AA is the floor for every surface, in both themes, verified independently — not
assumed from the other mode.** AAA (7:1) is the target for the primary body/ink pairings, which
we clear by a wide margin in both modes; AA (4.5:1 body text, 3:1 large text/UI components) is
the binding minimum for semantic colors and chart series, which run tighter by nature of being
distinguishable hues rather than pure neutrals.

Touch targets: minimum 44×44px on any interactive element regardless of visual size — a table
row's tap target extends to the full row even if the visible text is 13px mono.

## Verified contrast (computed, not eyeballed)

| Pairing | Dark | Light |
|---|---|---|
| `fg-ink` / `bg-canvas` | 18.2:1 | 16.1:1 |
| `fg-body` / `bg-canvas` | 13.4:1 | 9.5:1 |
| `fg-muted` / `bg-canvas` | 5.6:1 | 5.8:1 |
| `fg-ink` / `bg-surface` | 16.4:1 | 17.7:1 |
| `fg-body` / `bg-surface` | 12.1:1 | 10.4:1 |
| `fg-muted` / `bg-surface` | 5.0:1 | 6.3:1 |
| `color-success` / `bg-surface` | 8.8:1 | 5.1:1 |
| `color-warning` / `bg-surface` | 8.8:1 | 5.9:1 |
| `color-danger` / `bg-surface` | 5.4:1 | 5.4:1 |
| `color-info` / `bg-surface` | 7.1:1 | 5.2:1 |
| `fg-on-accent` / `color-accent` | 18.2:1 | 17.7:1 |
| chart-series-4 (violet) / `bg-surface` | 7.1:1 | 5.1:1 |
| chart-series-5 (teal) / `bg-surface` | 8.6:1 | 4.9:1 |
| chart-series-6 (rose) / `bg-surface` | 7.1:1 | 5.1:1 |

Every row clears 4.5:1; most clear 5:1+. `fg-muted` and the light-mode semantic colors are the
tightest pairings in the system — both were tuned specifically to clear AA (see
`08-dark-mode-strategy.md` for why light-mode warning/muted needed manual darkening beyond a
naive inversion of the dark-mode values).

## Non-color requirements

- **Never color alone.** Every semantic state (success/warning/danger/info) pairs with an icon
  or label, not just a hue — this also serves colorblind users and matches the terminal
  convention of a status chip carrying text ("OK", "FAULT"), not just a colored dot.
- **Focus-visible everywhere.** `:focus-visible` gets a 2px `color-accent` outline with 2px
  offset on every interactive element — no `outline: none` without a replacement.
- **Reduced motion.** See `05-motion.md` — all durations collapse to 0ms; ambient loops
  (shimmer, pulse) stop entirely.
- **Live regions.** Toasts use `aria-live="polite"`; a metric that updates live (e.g. current
  speed) uses `aria-live="off"` with the value exposed via a labeled element instead — an
  `aria-live` region firing on every telemetry tick would be an unusable screen-reader
  experience.
- **Keyboard.** Every table row, card, and chart data point that's clickable is a real
  `<button>`/`<a>`, reachable by Tab, activatable by Enter/Space — never a `div` with an
  `onClick`.

## Common mistakes

- Assuming a color pairing that passes in dark mode automatically passes in light mode (or vice
  versa) — amber and warm ambers in particular fail this reflexively; both modes are computed
  and listed above independently.
- Relying on `color-fg-muted` for anything the user must read to complete a task (an error
  message, a required field) — muted is for genuinely secondary text only; it's the tightest
  contrast ratio in the system on purpose.
- Shipping a status chip as color-only (a bare colored dot with no text) in a data-dense table
  where dozens of rows repeat the same dot — always pair with a short label.
