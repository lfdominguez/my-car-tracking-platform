# Motion

## Principle

Motion is purposeful and brief. It explains a change — where something came from,
what responded to you, what is still loading — and then gets out of the way. Data
values still *flash*; they don't animate. Everything below is `transform`/`opacity`
only (GPU-composited), and everything collapses under `prefers-reduced-motion`.

v2 widened the vocabulary from five presets to nine, because the product gained
things v1 didn't have: elevation (so controls can lift), routed page transitions,
and an ambient landing page. The ceiling is unchanged — a *state transition* stays
under ~450ms; anything longer is an ambient loop, and ambient loops are rationed.

| Preset | Duration | Easing | Behavior | Use |
|---|---|---|---|---|
| `press` | `duration-fast` (140ms) | `ease-standard` | translateY(-1px) on hover, `scale(.985)` on active | Every button and chip. The press is deliberately faster than the lift so the control feels mechanical rather than springy. |
| `sheen` | `duration-slow` (620ms) | `ease-standard` | a highlight gradient sweeps once across the face | Button hover only. One pass, never a loop. |
| `page-in` | `duration-entry` (420ms) | `ease-enter` | translateY(10px)→0, opacity 0→1 | Route change. Applied to `.page-view > *`, staggered 50ms per top-level block. |
| `stagger-in` | `duration-entry`, `duration-stagger-step` (45ms)/child | `ease-enter` | translateY(12px) + scale(.985) → none | Card grids and table rows on mount. Capped: past the 6th child everything shares one delay, so a 200-row table doesn't animate for six seconds. |
| `gauge-draw` | `duration-slow` | `ease-enter` | `stroke-dashoffset` from empty to the real reading | The dashboard fuel/charge ring. The keyframe declares only `from`, so the end state is whatever value the markup computed. |
| `badge-count-bump` | `duration-bump` (360ms) | `ease-spring-bump` | scale 1→1.25→1 | A metric counter changes value (odometer tick, live speed). Keyed to the new value. |
| `skeleton-shimmer` | `duration-ambient-shimmer` (1500ms) | `ease-linear` | gradient sweep, infinite | Loading placeholders only. |
| `pulse-attention` | `duration-ambient-pulse` (2400ms) | `ease-ambient` | dot opacity + expanding ring | Live/in-progress status dots. Slowed from 1200ms in v1 — at that rate it read as an alarm. One at a time. |
| `aurora-drift` | 22–32s | `ease-ambient` | slow translate + scale of a blurred orb | Marketing landing and the auth shell only. Never inside the app. |

Easings: `ease-standard` for interaction feedback, `ease-enter` for things arriving,
`ease-exit` for things leaving, `ease-spring-soft` for panels and drawers,
`ease-spring-bump` for a value that changed, `ease-ambient` for loops.

## Reduced motion

`tokens.css` zeroes every duration under `prefers-reduced-motion: reduce`. The polish
layer at the bottom of `style.css` then removes the *movement* itself — hover lifts,
translates, and scales resolve to `none` — leaving opacity changes, which stay legible
and cause no vestibular discomfort. Ambient loops (`aurora-drift`, `route-draw`,
`skeleton-shimmer`) are switched off entirely rather than sped up.

## Common mistakes

- **Animating a number's value.** Counting up from 0 to 184,320 km is decoration that
  delays the reading. The number appears; the container may fade in.
- **Adding a second ambient loop to a screen.** Two pulsing things compete and neither
  reads as urgent.
- **Transitioning `box-shadow` and `transform` at different speeds.** The lift and its
  shadow must arrive together or the card looks like it's peeling.
- **Staggering an unbounded list.** Cap the delay; a stagger that runs longer than about
  300ms total stops reading as choreography and starts reading as lag.
