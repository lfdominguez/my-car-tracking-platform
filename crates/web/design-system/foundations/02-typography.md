# Typography

## Principle

Two families, two jobs, never interchanged:

- **Inter** (sans, everything textual) — page titles, section headers, labels, nav, buttons,
  helper text, prose. It carries both the display and the body role, and that is deliberate:
  the surface is an instrument cluster, and the hierarchy is built from **size, weight and
  space**, not from a second voice. A display face was tried here (Domine, a serif) and
  removed — the editorial contrast fought the data rather than framing it.
- **JetBrains Mono** (mono, data) — every number that means something: odometer readings,
  fuel/energy figures, speeds, timestamps, coordinates, table cells, chart axis labels. Tabular
  figures throughout so a column of numbers aligns on the decimal point without manual padding.

`--font-family-display` and `--font-family-body` both resolve to Inter. The two token names
survive so a display face can be reintroduced later at a single point of change, without
touching any of the ~140 call sites.

Scale is dense at the bottom on purpose. Most text in this app lives between 11px and 15px,
so the ramp carries five rungs below the body size and widens only above it. The previous
ramp jumped 13px → 16px, and that single missing rung is what pushed ~40 declarations off the
scale into hand-picked rem values; `text-md` (14px) exists to close it.

## Tokens

| Token | Size | Line-height | Typical family | Use |
|---|---|---|---|---|
| `text-2xs` | 11px | `leading-relaxed` | body | Uppercase micro-labels, corridor mile markers |
| `text-xs` | 12px | `leading-normal` | mono / body | Badges, captions, chips |
| `text-sm` | 13px | `leading-normal` | mono / body | Secondary UI text, dense list rows |
| `text-md` | 14px | `leading-normal` | body | Table cells, panel titles, most helper text |
| `text-base` | 15px | `leading-normal` | body | Default body copy |
| `text-lg` | 17px | `leading-snug` | body | Card title, lead paragraph, large controls |
| `text-xl` | 20px | `leading-snug` | display | Section heading (h3-equivalent) |
| `text-2xl` | 24px | `leading-tight` | display | Page heading (h2-equivalent) |
| `text-3xl` | 30px | `leading-tight` | display / mono | Stat-panel hero numeral |
| `text-4xl` | 38px | `leading-tight` | display / mono | Rare — landing hero only |

Note `text-base` is 15px, not 16px. The rem root stays at 16px (`style.css`, `html, body`), so
every remaining `rem` value in the codebase is unaffected; `text-base` describes *UI text*, and
15px is where the bulk of the old hand-picked values actually sat.

Weights: `weight-display-{medium 500, semibold 600, bold 700}`,
`weight-body-{regular 400, medium 500, semibold 600, bold 700}`,
`weight-mono-{regular 400, medium 500, bold 700}`.
Never reach past these — no light/thin weights, no 800/900. Inter is variable across 100–900,
which makes it easy to invent a `650`; don't. The ramp is the ramp.

Letter-spacing is tighter than it was, because Inter sets tighter than the serif it replaced:
`tracking-tighter` (-0.022em) on headings and mono numerals, `tracking-tight` (-0.014em) as the
heading default, `tracking-normal` for body, `tracking-wide` (+0.045em) on uppercase micro-labels,
and `tracking-wider` (+0.08em) reserved for the two labels that genuinely want air
(`.nav-group-label`, `.landing-kicker`).

## Usage

```css
.view-title { font: var(--font-weight-display-semibold) var(--text-2xl)/var(--leading-tight) var(--font-family-display); letter-spacing: var(--tracking-tighter); }
.metric-hero { font: var(--font-weight-mono-medium) var(--text-3xl)/var(--leading-tight) var(--font-family-mono); font-variant-numeric: tabular-nums; }
.table-cell--numeric { font: var(--font-weight-mono-regular) var(--text-sm)/var(--leading-normal) var(--font-family-mono); font-variant-numeric: tabular-nums; text-align: right; }
.eyebrow { font: var(--font-weight-body-medium) var(--text-2xs)/1 var(--font-family-body); letter-spacing: var(--tracking-wide); text-transform: uppercase; }
```

## Delivery

Both faces are self-hosted variable `.woff2` files in `public/vendor/fonts/`, declared in
`public/fonts.css`, because the app's CSP is `font-src 'self' data:`. Inter is preloaded from
`index.html` since it is on the critical path for every view. `font-synthesis-weight: none` is
set on `html, body`: if Inter fails to load, the fallback should look like a plain system font
rather than a fake-bolded approximation of the real one.

## Common mistakes

- Writing a raw `font-size` in px or rem. Every size in `style.css` is a `var(--text-*)`, and
  the four exceptions (`.icon.sm/md/lg/xl`) are icon-glyph dimensions, not text.
- Putting a metric in Inter instead of JetBrains Mono. Any number a user compares against
  another number (trip A vs trip B, this month vs last month) must be mono + tabular, or the
  eye can't align digits.
- Mixing tabular and proportional figures in the same table column — pick tabular for the
  whole numeric column, always.
- Adding a `font-family` to a component to escape an inherited one. There is nothing to escape
  now; if a rule needs a family at all, it is a number and it wants `--font-family-mono`.
- Changing `CHART_FONT_UI` / `CHART_FONT_NUM` in `src/components/charts.rs` without changing
  the CSS tokens, or vice versa. ECharts renders to canvas and cannot read CSS custom
  properties, so those two constants are a hand-maintained mirror of the token values.
