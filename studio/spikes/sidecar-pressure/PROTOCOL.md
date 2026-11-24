# Spike: Apple Pencil → Sidecar → Krita pressure gate

Gate for the Bridge Studio carrier decision (design:
`docs/architecture.md`, Native surfaces).
If pressure does not reach Krita through iPad Sidecar on the reference
machine, the carrier decision reopens. Run **before** any pipeline
work.

Instruments (all stock Krita 5.3.x, no plugins):

1. **Tablet Tester** — Settings → Configure Krita → Tablet → "Tablet
   Tester". Drawing in its area shows red lines (mouse events) and
   blue lines (tablet events). Blue present ⇒ Krita receives tablet
   events; the right-side text output carries raw pressure values.
2. **Log Viewer docker** — Settings → Dockers → Log Viewer; enable
   logging, configure category "Tablet Handling". Backup evidence.
3. **Behavioral** — a size-mapped-to-pressure brush (e.g. "Basic-5
   Size") on a normal document: a light stroke and a heavy stroke must
   visibly differ in width.

## Protocol

1. User connects iPad via Sidecar and confirms the Krita window is on
   the iPad display (Pencil only acts on the iPad display).
2. Executor stages Krita: new document, Tablet Tester open.
3. User draws with Apple Pencil inside the Tablet Tester drawing area:
   one light stroke, one heavy stroke, one stroke ramping light→heavy.
4. Executor captures the tester text output and checks: blue lines
   present; `pressure=` values span a real range (not pinned at a
   constant).
5. User repeats the three strokes on the document canvas with
   "Basic-5 Size"; executor screenshots the result for the width
   difference.
6. Verdict + evidence recorded in `RESULTS.md`.

## Pass criteria

- Tablet Tester shows blue (tablet-event) lines under the Pencil, and
- raw pressure values in the text output vary across at least
  0.2–0.9 between the light and heavy strokes, and
- canvas strokes with a pressure-size brush show a clear width
  difference between light and heavy.

Any failure → carrier decision reopens (fallback per design: native
input path decision, possibly different carrier).
