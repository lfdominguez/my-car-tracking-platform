# COMPONENT: Chart — grouped bar

Side-by-side bars comparing categories across groups — e.g. fuel efficiency by car across the
last 4 weeks, or trip count by day-of-week across two cars. Rendered via the already-vendored
ECharts (`public/vendor/echarts.min.js`); this contract governs the design layer ECharts is
themed to match, not the charting library choice.

## Anatomy
`x-axis-categories`, `y-axis-value`, `grouped-bars`, `legend`, `value-labels` (optional, see
below), `hover-tooltip`.

## Variants
- `default` — up to 4 series per group before density becomes unreadable at typical card
  widths; beyond 4, prefer a `chart-series` line chart or a filtered view instead of cramming
  more bars in.
- `compact` — no axis labels/legend, used inline inside a small metric card as a glance-level
  visual (still requires a text alternative via `aria-label` summarizing the trend).

## States
`default`, `bar-highlighted` (hover/focus on one bar dims siblings to 60% opacity), `group-
isolated` (clicking a legend item isolates that series, others dim), `loading`
(`skeleton-loader-card`-style placeholder bars, static gray, no shimmer on the bars themselves
— shimmer reads as "data," a static block reads more honestly as "not yet loaded"), `no-data`
(`empty-state` treatment, full chart-area width).

## Props
```
categories: string[] (x-axis)
series: { name: string, values: number[], colorToken: chart-series-1..6 | success | warning | danger }[]
valueLabels: bool = false (show only when bar width allows without clipping, see below)
compact: bool = false
```

## Tokens used
`bg-surface` (chart background), `fg-body` (axis labels), `fg-muted` (gridlines, de-emphasized
elements), `color-chart-series-1..6` (default categorical assignment) or explicit
`success`/`warning`/`danger` when the chart is a threshold/status chart rather than a pure
comparison (see `foundations/11-data-viz.md`), `border-default` (plot area boundary, if any —
prefer no boundary box, let the hairline grid of the surrounding page do that job).

## A11y
- Every chart ships a `table view fallback` — a visually-hidden (or expand-to-reveal) real
  `<table>` with the same categories/series/values, for screen reader and non-visual access.
  ECharts canvas rendering has no native accessibility tree.
- Axis labels are real text in the DOM layer where possible (ECharts renders to canvas by
  default — use its SVG renderer mode for this product so labels remain selectable/inspectable
  text rather than canvas pixels).
- Color is never the only differentiator between series — legend labels and, where chart width
  allows, direct end-of-bar labels reinforce which series is which.

## Motion
- Mount: bars grow from baseline, `duration-entry`-family (320ms) with `40ms` per-group
  stagger (`stagger-list-children` token family), capped at 8 groups — beyond that, mount
  instantly (see `05-motion.md`'s stagger cap guidance).
- Hover tooltip: `duration-fast` fade, `ease-enter`.
- Value updates (e.g. live-refreshing week-to-date data): re-render bars to new height
  instantly, no grow animation on update — only the initial mount animates.

## Do / Don't
- Do: lock series-to-color assignment to a stable key (car ID, metric name) across re-renders
  and filters — never reassign by array index.
- Do: show value labels (`text-2xs` mono, tabular) only when the bar is wide enough not to
  clip; rely on tooltip otherwise.
- Don't: use more than 6 series in one grouped-bar chart — beyond `chart-series-6`, split into
  multiple charts or switch chart type.
- Don't: apply `success`/`warning`/`danger` coloring to a chart that's comparing categories
  rather than showing a threshold/status — see the semantic-vs-categorical rule in
  `foundations/11-data-viz.md`.
