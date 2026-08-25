# Dark-mode pairings

Full light↔dark token map. Every pairing was independently designed and contrast-checked (see
`foundations/07-accessibility.md` for the numbers, `foundations/08-dark-mode-strategy.md` for
the reasoning). None of these are computed by inverting the other mode's hex value.

| Token | Dark | Light | Independent rationale |
|---|---|---|---|
| `color-bg-canvas` | `#0a0a0a` | `#f4f4f5` | Dark canvas near-black with headroom above it for the elevation ladder; light canvas is a step *below* white so surfaces can sit visibly "up" against it |
| `color-bg-surface` | `#171717` | `#ffffff` | Dark surface is a lightness step up from canvas; light surface is the ceiling (white) — same elevation *concept*, opposite RGB direction |
| `color-bg-surface-raised` | `#1f1f1f` | `#ffffff` | Dark keeps climbing the ladder; light has hit its ceiling, so raised = same white + `border-strong` does the separating work instead |
| `color-bg-surface-overlay` | `#262626` | `#ffffff` | Same as above — light-mode overlays lean on `shadow-overlay` (the one permitted shadow) for separation since the lightness ladder has run out of headroom |
| `color-fg-ink` | `#f5f5f4` | `#18181b` | Max-contrast neutral per mode — not literally inverted values, but the same *role* (highest-contrast text) computed independently for each canvas |
| `color-fg-body` | `#d4d4d3` | `#3f3f46` | Mid-contrast neutral, picked to clear ≥9.5:1 on that mode's canvas, not derived from the other mode |
| `color-fg-muted` | `#888884` | `#5f5f66` | **Manually tuned** — the "obvious" light-mode midpoint gray (`#71717a`) only cleared 4.4:1; darkened to `#5f5f66` (5.8:1) to hold AA |
| `color-accent` | `#f5f5f4` (= ink) | `#18181b` (= ink) | "Primary" is redefined per mode as "maximum-contrast neutral," not a shared brand hex flipped in lightness |
| `color-success` | `#34d058` | `#1a7f37` | **Manually tuned** — dark-mode bright green clears ~2.5:1 on white; light value is a distinct deep green picked and verified against `bg-surface` white (5.1:1) |
| `color-warning` | `#f1a72b` | `#8a5b00` | **Manually tuned** — the pairing that needed the most correction; bright amber is illegible as text on white (~2.3:1), light value is a deep amber-brown (5.9:1) |
| `color-danger` | `#f85149` | `#cf222e` | Deepened for light-surface legibility (5.4:1 in both modes by design, not coincidence) |
| `color-info` | `#58a6ff` | `#0969da` | Same treatment — deep blue for light-mode text legibility (5.2:1) vs a lighter sky blue for dark-mode legibility (7.1:1) |
| `*-subtle` tints | 14% alpha of the dark hue | 10% alpha of the *light* hue (not the dark hue re-used) | Light-mode subtle fills derive from light mode's own deep text-hue, not a low-alpha wash of the dark-mode bright hue — avoids a washed-out/sickly tint |
| `color-border-default` | `rgba(245,245,244,.08)` | `rgba(24,24,27,.08)` | Same alpha, opposite base — hairline visibility tuned equal in both modes at that alpha |
| `color-overlay-scrim` | `rgba(10,10,10,.72)` | `rgba(24,24,27,.48)` | Dark needs a heavier scrim since the modal surface is close in lightness to the page behind it; light mode's white-on-gray already separates more, so a lighter scrim suffices |
| `chart-series-1..6` | bright/light-safe-on-dark hues | deep/light-safe-on-white hues | Each of the 6 categorical series independently re-picked per mode, not a single hue with alpha/lightness shifted — see `foundations/11-data-viz.md` |
