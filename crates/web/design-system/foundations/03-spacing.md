# Spacing

## Principle

Single unit, **4px**, 13 steps. 4 (not 8) because this is a dense, row-heavy product — an
8px floor would force every table row and metric chip to be visibly looser than the terminal
aesthetic calls for. Everything — padding, margin, gap — is a multiple of 4px; there is no
second scale and no arbitrary pixel value anywhere downstream.

Card padding deliberately runs tighter than typical marketing-surface guidance (24–40px).
`space-card-pad` is 16px. This is intentional, not an oversight: `VISUAL_DENSITY=5` here
targets a calm-but-dense dashboard, not a landing page, and `data-viz-dense`'s own instruction
is that "every pixel earns its place." If a card ever needs to feel more spacious (e.g. a
single-KPI hero card on the overview page), that is a one-off layout decision using `space-6`
directly — it does not get promoted to a second semantic card-padding token.

## Tokens

| Token | Value | | Token | Value |
|---|---|---|---|---|
| `space-0` | 0px | | `space-8` | 32px |
| `space-1` | 4px | | `space-10` | 40px |
| `space-2` | 8px | | `space-12` | 48px |
| `space-3` | 12px | | `space-16` | 64px |
| `space-4` | 16px | | `space-20` | 80px |
| `space-5` | 20px | | `space-24` | 96px |
| `space-6` | 24px | | | |

Semantic layer:

| Token | Maps to | Use |
|---|---|---|
| `space-row-y` | `space-1` (4px) | Table/list row vertical padding |
| `space-row-x` | `space-3` (12px) | Table/list row horizontal padding |
| `space-card-pad` | `space-4` (16px) | Card/panel interior |
| `space-panel-gap` | `space-2` (8px) | Gap between adjacent dashboard panels |
| `space-section-y-mobile` | `space-8` (32px) | Vertical rhythm between page sections, mobile |
| `space-section-y-desktop` | `space-12` (48px) | Vertical rhythm between page sections, desktop |
| `space-gutter-mobile` | `space-4` (16px) | Page/content horizontal margin, mobile |
| `space-gutter-desktop` | `space-6` (24px) | Page/content horizontal margin, desktop |
| `space-stack-tight` | `space-1` (4px) | Label-to-value, icon-to-label |
| `space-stack-loose` | `space-4` (16px) | Between unrelated paragraphs/blocks |

## Usage

```css
.data-table td { padding: var(--space-row-y) var(--space-row-x); }
.metric-card { padding: var(--space-card-pad); display: grid; gap: var(--space-stack-tight); }
.dashboard-grid { display: grid; gap: var(--space-panel-gap); }
.page-section + .page-section { margin-top: var(--space-section-y-mobile); }
@media (min-width: 768px) {
  .page-section + .page-section { margin-top: var(--space-section-y-desktop); }
}
```

## Common mistakes

- Reaching for a raw pixel value ("this needs 10px") instead of rounding to `space-2`(8) or
  `space-3`(12). If neither works, the layout problem is upstream, not a spacing gap.
- Applying marketing-scale card padding (`space-8`/32px+) to a dashboard panel — it reads as
  wasted density on a product whose entire value proposition is "see everything at once."
- Using `space-row-y` (4px) outside of dense table/list rows — it's too tight for anything
  that isn't a repeating data row.
