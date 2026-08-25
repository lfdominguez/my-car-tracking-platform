# COMPONENT: Table

The core `data-viz-dense` component — trip history, telemetry logs, car lists. Desktop-only
presentation; below `breakpoint-md` it hands off to the mobile row-card pattern described in
`foundations/06-responsive-density.md` (same data, different component — see
`empty-state.contract.md` for the shared empty treatment across both).

## Variants
- `default` — 0.5px `border-default` between every row and column, `bg-surface` row background,
  `bg-surface-raised` on row hover.
- `sticky-header` — header row pins to the top of the scroll container (`position: sticky`),
  used on any table taller than ~8 rows (trip history, telemetry log).
- `sortable` — header cells become buttons with a sort-direction glyph; active sort column gets
  `fg-ink` weight, inactive columns `fg-muted`.

## States
| State | Treatment |
|---|---|
| default | populated rows |
| row-hover | `bg-surface-raised`, `duration-instant` (this fires constantly while scanning a dense table — must be free, not eased) |
| loading | `skeleton-loader-card`-style row placeholders matching column widths, `aria-busy="true"` on the table |
| empty | full-width empty state row (never collapses to a blank table with just a header) — see `empty-state.contract.md` |
| error | full-width error row with retry action, same width as empty state |

## Props
```
columns: Column[] (each: key, label, align: left | right, numeric: bool, sortable: bool)
rows: T[]
sortKey: string?
sortDirection: asc | desc?
onSort: (key) => void
stickyHeader: bool = true
```

## A11y
- Real `<table>`/`<thead>`/`<tbody>`/`<th scope="col">` markup — never a `div`-grid pretending
  to be a table; screen reader table navigation depends on real semantics here.
- Sortable header buttons announce current sort state via `aria-sort` on the `<th>`
  (`ascending`/`descending`/`none`).
- Row click targets (when rows are interactive/navigable) are real `<a>`/`<button>` wrapping the
  row content, not a row-level `onClick` with no keyboard path.

## Motion
- Row hover: `duration-instant` (0ms) — see `05-motion.md`'s `instant-exit` rationale; hover
  feedback on a table you're scanning at speed must not lag behind the cursor.
- Sort re-order: no animated row reordering (would violate the transform-only/no-layout-thrash
  rule at table scale) — rows re-render in new order instantly.
- New row insertion (e.g. a live trip appearing): `stagger-list-children` treatment on insert
  only, capped to the single new row, not the whole table.

## Slots
`toolbar` (optional — filters, search, column visibility toggle above the table),
`header-row`, `body-rows`, `empty-row`, `pagination` (below the table, right-aligned per
"action columns right-aligned" convention).

## Do / Don't
- Do: right-align every numeric column, `font-family-mono`, tabular figures — this is the
  single most load-bearing visual convention in the whole system for this product.
- Do: keep the empty state at full table width, not a collapsed single line — an empty trip
  history table should explain *why* it's empty (no car selected vs. genuinely zero trips) and
  offer the next action.
- Don't: use client-side infinite scroll and virtualized rows in the same table without an
  explicit row-count indicator — dense tables need the user to know how much data exists.
- Don't: hide the header row on mobile scroll without pinning it — a 20-column-wide dense table
  scrolled horizontally with no visible header is unusable; prefer the mobile row-card
  transform from `06-responsive-density.md` over a horizontally-scrolling dense table.
