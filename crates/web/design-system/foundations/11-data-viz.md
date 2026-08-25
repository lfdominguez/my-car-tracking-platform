# Data visualization: charts & maps

## Principle

This is the doc that matters most in this system — metrics, graphs, and maps are first-class
product content, not supporting decoration. Two hard rules govern every chart and map:

1. **Semantic color (success/warning/danger/info) means a state claim. Categorical color
   (`chart-series-1`–`6`) means "this is a different thing," with no judgment attached.** A
   fuel-efficiency gauge coloring green/amber/red by threshold uses semantic tokens. A chart
   comparing five different cars' trip distances uses the categorical sequence — car #2 being
   `chart-series-2` (amber-family) is not a claim that car #2 is doing something wrong.
2. **Every number is JetBrains Mono, tabular, and every chart/map ships all four states** —
   loading (skeleton, matching final shape), empty (no-data, see `no-results-state` contract),
   error (named failure — "GPS data unavailable for this trip," never a bare error icon), and
   populated.

## Chart color sequence

Six categorical series, tuned independently per mode (see `08-dark-mode-strategy.md`) so each
stays ≥4.5:1 against `bg-surface`:

| Token | Dark | Light | Notes |
|---|---|---|---|
| `chart-series-1` | `#58a6ff` | `#0969da` | Default first series — same hue family as `info` |
| `chart-series-2` | `#f1a72b` | `#8a5b00` | Same hue family as `warning` — fine as series #2, not as a state claim |
| `chart-series-3` | `#34d058` | `#1a7f37` | Same hue family as `success` |
| `chart-series-4` | `#bc8cff` | `#8250df` | Violet — no semantic overlap, safe for pure comparison charts |
| `chart-series-5` | `#39c5cf` | `#1b7c83` | Teal |
| `chart-series-6` | `#f778ba` | `#bf3989` | Rose |

Assign in fixed order (series 1 always gets `chart-series-1`, regardless of which car/metric it
is) so a returning user's muscle memory for "blue = my car" stays consistent across visits.
When a chart is inherently a status/threshold chart (efficiency band, health score), use
`success`/`warning`/`danger` directly instead of the categorical sequence — don't reuse
`chart-series-2` as a fake "warning" because it happens to be amber.

**Grouped bar chart** (`chart-bar-grouped`, prioritized component): bars grow from baseline on
mount with a `40ms`-per-group `stagger-list-children`-family stagger, capped at 8 groups; beyond
that, mount instantly. Value labels render in `text-2xs` mono, tabular, only when the bar is
wide enough not to clip (hide below ~24px bar width, rely on tooltip instead).

## Map styling

MapLibre GL is already vendored (`public/vendor/maplibre-gl.{js,css}`). Two basemap styles, one
per theme, swapped by the same `data-theme` mechanism as the rest of the UI:

- **Dark basemap**: near-black water/land fill matched to `bg-canvas`/`bg-surface`, muted
  low-contrast road lines (`fg-muted`-equivalent), labels in `fg-body`, no colored land-use
  fills (parks/water get lightness-only differentiation, not hue) — the map should read as an
  extension of the terminal, not a full-color tourist map dropped into a dark UI.
- **Light basemap**: same structure inverted — light land fill, `fg-muted`-equivalent roads,
  `fg-body` labels.

| Token | Use |
|---|---|
| `map-route` | Default trip polyline color |
| `map-route-active` | Selected/hovered trip segment — always `color-accent` (max contrast) so the active route is unambiguous against any basemap |
| `map-marker-start` | Trip start pin — always `success` green (wayfinding convention: green = go/start) |
| `map-marker-end` | Trip end pin — always `danger` red (convention: red = stop/end) — this is a wayfinding convention, not a "something went wrong" claim; document this distinction in any onboarding copy near the map |
| `map-marker-poi` | Neutral points of interest (charging stations, service stops) | 

Route polylines: 3px stroke on desktop, 4px on mobile (touch-target legibility), rendered under
markers, `map-route-active` always renders in front of `map-route` for overlapping segments.

## Common mistakes

- Auto-assigning chart series colors by array index in a way that changes between renders (e.g.
  after a filter) — lock series-to-color assignment to a stable key (car ID, metric name), not
  array position.
- Using `map-marker-end` (danger red) to imply the trip ended badly — it's a wayfinding
  convention (start=green/end=red), not a status signal; don't reuse it for "trip had a fault,"
  which should get its own distinct marker treatment (e.g. a warning-colored marker with a
  fault icon).
- Rendering full-color third-party map tiles (satellite, full-saturation vector styles) inside
  an otherwise `data-viz-dense` dark UI — breaks the surface's tonal discipline; always use the
  muted basemap variants above.
- Skipping the loading/empty/error states on a chart because "it usually has data" — a GPS
  dropout or a car with zero trips is a real, common case for this product.
