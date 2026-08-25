# COMPONENT: Input

## Variants
- `text`, `email`, `phone`, `number`, `search`, `textarea`.
- `with-icon` — leading icon (e.g. magnifying glass for `search`, car glyph for a VIN field).
- Numeric inputs (`number`, and any field capturing a metric — odometer override, fuel price)
  render their *value* in `font-family-mono` with tabular figures even though the field chrome
  uses `font-family-body`, matching the rest of the system's number treatment.

## States
| State | Treatment |
|---|---|
| default | `bg-surface` fill, `border-default` (0.5px), `radius-surface` (0) |
| hover | `border-strong` |
| focus | `border-strong` + 2px `color-accent` outline (offset 0, since the input border itself is the boundary) |
| filled | no visual change from default beyond the value being present — filled state is not a separate treatment |
| disabled | `opacity: 0.5`, `fg-muted` value text, `cursor: not-allowed` |
| error | `danger` border (1px, replacing the hairline), error icon in trailing slot, helper text swaps to `danger` |
| loading (async validation) | trailing `spinner-inline`, input remains editable unless the action truly blocks (e.g. checking VIN uniqueness) |

## Props
```
type: text | email | phone | number | search | textarea = text
label: string (required — never placeholder-only)
placeholder: string?
helper: string? (rendered even when empty — reserves layout space so an error appearing doesn't shift the row below)
error: string?
disabled: bool = false
required: bool = false
leadingIcon: IconName?
```

## A11y
- Visible `<label>` always present, programmatically associated (`for`/`id`), never
  placeholder-only.
- Helper text slot exists in markup even when empty (`min-height` reserved) so an error
  appearing doesn't cause layout shift in a dense form.
- Error text is announced via `aria-describedby` pointing at the error element; the input
  gets `aria-invalid="true"` when in error state.
- `required` fields marked with both a visual indicator and `aria-required="true"` — not
  color alone.

## Motion
- Border/outline transitions: `duration-fast`, `ease-enter`.
- Error state appearing: no animation — appears instantly (form validation is high-frequency
  state churn, see `instant-exit` guidance in `05-motion.md`) paired with the helper-text slot
  already reserving space so nothing jumps.

## Slots
`label`, `leading-icon`, `input`, `trailing-icon` (validation spinner or error glyph),
`helper`, `error`.

## Do / Don't
- Do: reserve helper-text height in the layout at all times, filled or not.
- Do: use mono/tabular rendering for any numeric field value.
- Don't: use placeholder text as the only label — placeholder disappears on input and fails
  screen readers and older-driver low-vision use cases both.
- Don't: validate on every keystroke for anything beyond simple format checks (email shape) —
  debounce async validation (VIN lookup, uniqueness checks) to avoid spinner flicker.
