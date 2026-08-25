# Motion

## Principle

`data-viz-dense` motion is not "reduced" — it's *purposeful and brief*. Data updates flash,
they don't animate. Five presets, hand-picked against `motion-presets.json` for this exact
product, cover every motion need in the system; nothing outside this list ships. All are
`transform`/`opacity` only (GPU-composited), respect `prefers-reduced-motion`, and are named
after their source preset id for traceability.

| Preset token | Duration | Easing | Behavior | Use |
|---|---|---|---|---|
| `instant-exit` | `duration-instant` (0ms) | `ease-linear` | opacity 1→0, no transition | High-frequency state churn: autocomplete rows filtering out, validation flashes. Anything that would otherwise animate dozens of times a second. |
| `skeleton-shimmer` | `duration-ambient-shimmer` (1500ms) | `ease-linear` | gradient sweep, infinite loop | Loading placeholders only. The one duration that exceeds the 1200ms micro-motion ceiling — permitted because it's an ambient loop, not a state transition. |
| `badge-count-bump` | `duration-bump` (360ms) | `ease-spring-bump` | scale 1→1.25→1 | A metric counter changes value (odometer tick, live speed update, trip count increment). Keyed to the new value so each change gets its own bump. |
| `stagger-list-children` | `duration-entry` (320ms), `duration-stagger-step` (40ms) per child | `ease-enter` | translateY(8px)→0, opacity 0→1 | Row/list reveal on mount — trip list, car list, corridor list. Cap stagger at ~8 visible children; beyond that, only stagger what's in the viewport. |
| `pulse-attention` | `duration-ambient-pulse` (1200ms) | `ease-ambient` | scale 1→1.05→1, opacity 1→0.6→1 | Sparing use — one anomaly flag on one metric at a time (e.g. a fault code, an efficiency outlier). Never more than one pulsing element on screen. |

There is no generic "hover transition" token beyond `duration-fast` (150ms) — used for the
smallest interaction feedback (button background, link underline) with `ease-enter`.
`duration-medium` (250ms) covers component-level transitions that aren't one of the five named
presets above (e.g. a panel expanding).

## Reduced motion

`tokens.css` collapses every non-zero duration to `0ms` under `prefers-reduced-motion: reduce`
globally — components don't need their own media query for duration, only for whether a
transform-based effect should be replaced with a plain state swap (e.g. shimmer becomes a
static gray block; pulse-attention becomes a static colored border instead of animating).

## Usage

```css
.metric-value[data-updated] { animation: count-bump var(--motion-duration-bump) var(--motion-ease-spring-bump); }
@keyframes count-bump { 0%, 100% { transform: scale(1); } 40% { transform: scale(1.25); } }

.list-item { animation: fade-up-8 var(--motion-duration-entry) var(--motion-ease-enter) both; }
.list-item:nth-child(n) { animation-delay: calc((var(--index, 0)) * var(--motion-duration-stagger-step)); }
```

## Common mistakes

- Animating a data update that happens more than once a second (a live speed readout) with
  anything other than `instant-exit` or a plain value swap — a 250ms ease on a value that
  changes 4x/second reads as lag, not motion.
- Running `pulse-attention` on more than one element at a time — it stops meaning "look here"
  and starts meaning "the page is nervous."
- Adding a sixth motion preset for a one-off need. If nothing in this table fits, the answer is
  usually "this doesn't need motion," not "invent a new duration."
