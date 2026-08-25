# Breakpoints & responsive density strategy

## Principle

`data-viz-dense`'s own manifest lists "low-density mobile-first UIs" under `when_to_skip`. This
product must be both — dense on desktop *and* genuinely mobile-first, because mobile-support and
minimal-clicks-to-key-data are both must-haves in the brief. We do not resolve this by shrinking
the dense grid until it technically fits a 360px viewport (that produces 8px unreadable mono
text and horizontal scroll). We resolve it by **changing what's on screen per breakpoint**, not
just how big it is: mobile shows fewer panels and fewer columns, at full readable density,
drilling down for the rest.

Think of it as three different information-density modes of the *same* data, not one layout
resized:

| Breakpoint | Range | Density mode |
|---|---|---|
| Mobile | `< 768px` (`breakpoint-md`) | **Drill-down.** One panel/table at a time. Table rows collapse to a single-line summary card (primary metric + delta + status chip); tapping a row navigates to the full detail view rather than expanding inline. Charts show one series at a time with a segmented control to switch, not six overlaid lines. |
| Tablet | `768–1023px` (`breakpoint-md`–`breakpoint-lg`) | **Priority columns.** Two-panel max (e.g. list + one detail pane, not list + detail + map simultaneously). Tables show 3-4 priority columns; secondary columns move to an expandable row detail. |
| Desktop | `≥ 1024px` (`breakpoint-lg`) | **Full terminal density.** Multi-panel dashboard, full table columns, multiple simultaneous chart series, side-by-side map + metrics. This is the `data-viz-dense` baseline the style was designed for. |

## Tokens

| Token | Value |
|---|---|
| `breakpoint-sm` | 480px |
| `breakpoint-md` | 768px |
| `breakpoint-lg` | 1024px |
| `breakpoint-xl` | 1280px |
| `breakpoint-2xl` | 1536px |

CSS custom properties can't be read inside `@media` conditions — these tokens are the source of
truth for the pixel values; every `@media` rule in `style.css` and component styles must
hardcode the same numbers. Keep this file and the media queries in sync by hand; there is no
build step generating one from the other in a vanilla-CSS stack.

## Usage

```css
/* Desktop: full data table */
.trip-table { display: table; }
.trip-card { display: none; }

/* Mobile: single-line drill-down card instead of a wide table */
@media (max-width: 767px) {
  .trip-table { display: none; }
  .trip-card { display: flex; align-items: center; justify-content: space-between; }
}
```

```css
/* Desktop: 3-panel dashboard (nav rail + list + map) */
.dashboard-grid { grid-template-columns: 240px minmax(0, 1fr); }

@media (min-width: 1024px) {
  .dashboard-grid { grid-template-columns: 240px minmax(280px, 380px) minmax(0, 1fr); }
}
```

## Common mistakes

- Shipping the desktop 6-column table at `font-size: 10px` on mobile instead of switching to
  drill-down cards. Undersized mono type on a phone is the density tension made literal — it's
  the exact failure mode the style's own `when_to_skip` warns about.
- Showing the map, the metrics panel, and the trip list simultaneously below `768px` — pick the
  single panel the user needs first (usually the list) and route to the rest.
- Forgetting that "drill-down" still needs to be fast — a mobile row tap should never take more
  than one navigation to reach the metric the user came for. Minimal-clicks is a must-have at
  every breakpoint, not just desktop.
