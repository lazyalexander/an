# Software-bridge floor-loop spike — report

Date: 2026-11-22
Branch: `dev-software-bridge` (independent line; not merged)
Status: proven — plan executed 9/9, review round closed
Implements: `docs/superpowers/plans/2026-11-22-software-bridge-spike.md`
Spec: `docs/superpowers/specs/2026-11-22-software-bridge-design.md`

## What was built

The floor loop from the spec: a Python spine watches a source
application's window (5 Hz window capture, frame diffing, settle
detection), and on "settled" delivers the captured view into a painting
web app as a named layer, replacing it in place on later pushes. Every
crossing is recorded in a JSONL ledger with bytes and wall-clock
latency.

The spike code lives in the gitignored lab area by design (lab code
does not enter the product repo; the execution ledger and this report
are the record). Unit suite: 14/14 passing, including a test that
greps the framework layer for domain words — the spine contains no
application names and no domain vocabulary; all application specifics
live in adapters.

## Verification results (manual pass, macOS, Retina)

- **Happy path**: five clean settle→push samples, push latency
  1192–1227 ms (median 1207 ms, including sink-side DOM automation);
  3.3–3.6 MB Retina PNG per artifact; last source motion to layer
  landed ≈ 1.6–1.9 s — inside the spec's ~2 s target.
- **Animation suppression**: a full-window CSS animation at ~170× the
  change epsilon produced zero pushes while moving, exactly one after
  stopping.
- **Occlusion isolation**: with the source window half covered,
  captured pixels in the covered region remained the source's own —
  capture reads the window, not the screen.
- **Retina**: a 1728×1083 pt window captured at 3456×2166 px, exact 2×.
- **Source quit/relaunch**: exactly one `failed` ledger entry (only on
  the ok→missing transition); relaunch auto-resumes on the next settle.
- **Sink tab loss**: with the sink tab closed, the next settle reopened
  the app tab and pushed successfully (end-to-end 1571 ms).
- **Readback**: post-push canvas read returns the pushed content
  (dominant color = source theme gray).

## Review round

Independent code review after the verification pass: verdict "with
fixes" — 1 Critical, 4 Important, 8 Minor. All landed:

- **Critical — unbounded frame growth.** Every watch tick wrote a fresh
  multi-MB PNG with no deletion (~10 GB in ~20 min of observing). Fixed
  by rotation: only the diffing pair and the last pushed artifact
  survive; snapshots overwrite a single file. Re-verified live:
  directories stay flat.
- **Important — domain-word violation in the framework layer.** The
  plan's own interface named a parameter with a forbidden domain word,
  and the word-boundary regex in the enforcement test masked it. Fixed
  by rename; ruling recorded. Lesson: deviations need a recorded ruling
  at the moment they are made, not at review time.
- **Important — recovery coverage.** The wedged-renderer reset-retry
  now wraps the import call, not just the install; the window watcher's
  baseline resets across source loss (pinned by a unit test); the
  ledger tolerates a crash mid-write (truncated line skipped, append
  repairs the missing newline first).
- **Recorded deferral**: per-push snapshots copy the pushed source
  image, not a canvas readback; painter strokes are not yet preserved
  (belongs to the adapter milestone, where `read_canvas` and the
  pose-keyed archive live).

## Discoveries worth keeping

- The bundled painting app exposes no scripting handle — import is
  driven through the same DOM surfaces a user touches (synthetic drop,
  modal buttons, layers panel). Same-name replace = delete matching
  rows, then re-import; layer count stays flat across pushes.
- Its easel repaints from a `requestAnimationFrame` loop: a hidden
  window shows a stale picture while the model stays current. The
  bridge window launches with anti-throttling flags, and `read_canvas`
  refuses to read while hidden.
- A reloaded renderer can wedge silently: the protocol responds, JS
  evaluation never returns. Timeouts plus session rebuild are the fix;
  retries alone hang forever.
- Input delivery to the source app from synthetic OS events is flaky
  and focus-sensitive. For verification this was a harness problem, not
  a bridge problem — the bridge only observes. Consequence below.

## Lessons handed to the next milestone

1. **Drive the source deterministically; keep the bridge's observation
   uncooperative.** The two are orthogonal. Future verification drives
   the modeler through its own scripting interface (or a
   self-animating dummy app) so motion amplitude and timing are known;
   OS-level synthetic input is the last resort of the interface
   hierarchy, including for test harnesses.
2. **Recovery paths need fault injection, not one reproduction.** The
   renderer wedge is a race; it was verified against an equivalent
   induced failure (manual tab close), not a recurring original.
3. **Any unbounded write is questioned the day it is written.** The
   disk-growth Critical was invisible to minutes-long verification runs
   and caught by review; a ten-minute soak would have caught it too.
4. **Measurement environments are exclusive or modeled.** A user
   sharing the desktop during a run is either contamination or an
   explicitly designed test dimension ("does the bridge degrade while
   the user works?") — never an accident.

## Honest limits (unchanged from the spike README)

- Delivered layer is a flattened whole-window screenshot, UI chrome
  included. Per-part decomposition arrives with the dedicated modeling
  adapter.
- Pose key is a perceptual hash of the screenshot — a proxy for "same
  view", not a camera pose.
- The sink's display is only fresh while its window is onscreen;
  `read_canvas` returns the displayed composite, letterboxed by zoom.
- Sink-window lookup is title-substring first-match; it may read (never
  touch) a user's own browser window if it shows the same site.

## Next

The dedicated modeling-app adapter milestone: exact camera pose,
per-part offscreen renders, native layer insertion — replacing every
floor-tier proxy listed above. That work is a new branch; this spike
branch stays as the record of the floor being proven.
