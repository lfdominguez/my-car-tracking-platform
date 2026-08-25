# Voice & tone

## Principle

Audience is broad consumer — car enthusiasts and everyday drivers, not fleet ops managers —
who want to know what's happening with their car in as few clicks as possible. The tone target
from the brief is **efficient, precise, data-forward, low-friction**. That means: sound like the
instrument cluster of a well-built car, not like a SaaS onboarding email. Numbers do the talking;
copy exists to label the number, not to sell it.

- **Declarative, not hedging.** "Fuel economy dropped 12% this week" — not "It looks like your
  fuel economy might have changed recently."
- **Name the number, not the vibe.** "312 km since last fill" beats "Great mileage!" Every claim
  in the product is backed by a real figure sitting right next to it.
- **No exclamation marks, no celebration language.** A completed trip is "Trip saved," not
  "Nice drive! 🎉" — this product is read daily, at a glance; enthusiasm language fatigues fast
  and reads as noise next to a terminal-density layout.
- **Errors name the field and the fix.** "GPS signal lost at 14:32 — route may be incomplete,"
  not "Something went wrong." A telemetry product especially can't hide behind vague errors;
  users are troubleshooting a physical device.
- **Bilingual from the start (es/en).** Copy is written to translate cleanly — avoid idioms,
  avoid compound wordplay in labels/buttons. Numbers, units, and dates follow locale
  (`km`/`mi`, `L`/`gal`, comma/period decimal separators) — this is a data correctness
  requirement, not just a translation nicety.

## Microcopy patterns

| Context | Do | Don't |
|---|---|---|
| Empty state (no trips yet) | "No trips logged yet. Drive with your OBD logger connected and trips appear here automatically." | "No data yet!" |
| Loading | "Loading trip history…" (paired with skeleton, not spinner-only, for anything >1s) | Bare spinner with no label |
| Success | "Trip saved" / "Settings updated" | "Awesome, all done! ✨" |
| Anomaly flag | "Engine temp 8°C above normal for this route" | "Uh oh, something looks off" |
| Destructive confirm | "Delete this trip? This can't be undone." | "Are you sure?" |

## Common mistakes

- Writing dashboard copy like marketing copy ("Unlock powerful insights into your driving!") —
  this is a utility the user checks daily; it should read like a utility.
- Translating es/en labels literally without checking length — Spanish UI strings run 15-30%
  longer than English on average; short labels (nav items, button text) need to be written to
  survive that expansion at `text-xs`/`text-sm` sizes without wrapping.
- Using first-person plural ("We think you'll love this trip") — describe the car/trip/data
  directly instead.
