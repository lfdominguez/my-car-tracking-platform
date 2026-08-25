# COMPONENT: Form field

The wrapper that coordinates label + control + helper + error for any input-like control
(text input, select, checkbox group, radio group) so every form in the app gets identical
spacing and error behavior without re-deriving it per field.

## Variants
- `stacked` (default) — label above control, full width. Used in almost every form in this
  product (settings, car profile, trip edit).
- `inline` — label left, control right, used only in dense settings rows
  (see `settings-section-grouped.contract.md`) where the row itself provides the label context.

## States
`default`, `focused` (control inside has focus — field wrapper gets no extra treatment beyond
what the control itself shows), `error`, `disabled`.

## Props
```
label: string (required)
helper: string?
error: string?
required: bool = false
layout: stacked | inline = stacked
```

## A11y
- Owns the `<label for>`/`id` relationship and the `aria-describedby` wiring to helper/error —
  child controls (`input`, `select`, etc.) don't re-implement this individually.
- Error text takes over the `aria-describedby` slot when present; helper text is hidden from
  the accessibility tree (not just visually) while an error is showing, so screen reader users
  don't hear both.

## Motion
None on the wrapper itself — motion belongs to the child control (see `input.contract.md`).

## Slots
`label`, `required-indicator`, `control` (any input-like child), `helper`, `error`.

## Do / Don't
- Do: use `form-field` for every labeled control in the app, including checkboxes/selects —
  don't hand-roll label+helper+error per form.
- Do: cap forms at 3-4 fields per screen/step for anything resembling a signup or onboarding
  flow, per the system's minimal-clicks/low-friction voice target.
- Don't: nest a `form-field` inside another `form-field` — compose at the layout level instead.
