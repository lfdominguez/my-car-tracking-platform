# Typography

## Principle

Three families, three jobs, never interchanged:

- **Domine** (serif, display) — page titles, one KPI hero numeral per view, section headers.
  Used *surgically*: it is the one editorial gesture inside an otherwise Bloomberg-terminal
  surface, and that contrast is deliberate — it's what keeps this reading as a car-tracking
  product for people, not a clone of a trading desk. It never appears below `text-lg` and
  never in a table.
- **DM Sans** (sans, body/UI) — everything that isn't a headline or a number: labels, nav,
  buttons, helper text, prose.
- **JetBrains Mono** (mono, data) — every number that means something: odometer readings,
  fuel/energy figures, speeds, timestamps, coordinates, table cells, chart axis labels. Tabular
  figures throughout so a column of numbers aligns on the decimal point without manual padding.

Scale is a true modular ratio, **1.2 (minor third)**, base 16px — the ratio built for
dense, data-heavy UI rather than a marketing type ramp. Eight steps, rounded to sane pixel
values after computing the ratio.

## Tokens

| Token | Size | Line-height | Typical family | Use |
|---|---|---|---|---|
| `text-2xs` | 11px | 16px (`leading-relaxed`≈1.45) | mono | Micro tags, corridor mile markers |
| `text-xs` | 12px | `leading-normal` | mono / body | Table cells, captions, badges |
| `text-sm` | 13px | `leading-normal` | mono / body | Secondary UI text, dense list rows |
| `text-base` | 16px | `leading-normal` | body | Default body copy |
| `text-lg` | 19px | `leading-snug` | body | Lead paragraph, card title |
| `text-xl` | 23px | `leading-snug` | display | Section heading (h3-equivalent) |
| `text-2xl` | 28px | `leading-tight` | display | Page heading (h2-equivalent) |
| `text-3xl` | 33px | `leading-tight` | display | Rare — top-level view title |
| `text-4xl` | 40px | `leading-tight` | display / mono | Hero KPI numeral, one per view max |

Weights: `weight-display-{medium 500, semibold 600, bold 700}`,
`weight-body-{regular 400, medium 500, bold 700}`, `weight-mono-{regular 400, medium 500, bold 700}`.
Never reach past these — no light/thin weights, no 800/900.

Letter-spacing: `tracking-tighter` (-0.02em) on `text-3xl`/`text-4xl` display only,
`tracking-tight` (-0.01em) on `text-xl`/`text-2xl`, `tracking-normal` for body,
`tracking-wide` (+0.08em) exclusively on uppercase eyebrow labels and mono table headers.

## Usage

```css
.view-title { font: var(--font-weight-display-semibold) var(--text-2xl)/var(--leading-tight) var(--font-family-display); letter-spacing: var(--tracking-tight); }
.metric-hero { font: var(--font-weight-mono-medium) var(--text-4xl)/var(--leading-tight) var(--font-family-mono); font-variant-numeric: tabular-nums; }
.table-cell--numeric { font: var(--font-weight-mono-regular) var(--text-sm)/var(--leading-normal) var(--font-family-mono); font-variant-numeric: tabular-nums; text-align: right; }
.eyebrow { font: var(--font-weight-body-medium) var(--text-xs)/1 var(--font-family-body); letter-spacing: var(--tracking-wide); text-transform: uppercase; }
```

## Common mistakes

- Setting Domine below `text-lg` — it was chosen for weight-bearing moments, not for body
  copy; small serif reads as an accessibility regression on a data product.
  Domine has no genuinely legible weight under ~18px at typical screen DPI.
- Putting a metric in DM Sans instead of JetBrains Mono. Any number a user compares against
  another number (trip A vs trip B, this month vs last month) must be mono + tabular, or the
  eye can't align digits.
- Mixing tabular and proportional figures in the same table column — pick tabular for the
  whole numeric column, always.
