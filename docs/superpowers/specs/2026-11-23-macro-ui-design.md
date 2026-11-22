# Macro UI — show only what the user wants to see

Date: 2026-11-23
Branch: `dev-macro-ui` (stacks on PR #2 `dev-bridge-studio`; retarget to
`dev` once #2 merges)
Status: draft — builder check resolved, pending final approval
Builds on: `docs/superpowers/specs/2026-11-22-bridge-studio-design.md`

> **Revision note (2026-11-23, builder check).** A builder check raised
> six questions; three were settled by the author (marked *author's
> call* below) and three were answered by the user:
>
> 1. **Adopt / re-render semantics (user).** Re-render is cumulative —
>    the painter's undo, the writer's revision: it never discards, and
>    going back is always possible. See *Content inventory*.
> 2. **Docker relationship (user).** The web page becomes the sole
>    control surface. This supersedes the parent spec's "the docker is
>    our only UI" and relaxes its "no use of the user's own browser"
>    anti-goal. See *Relationship to the Krita docker*.
> 3. **Behavior inference scope (user).** Record-only in this
>    milestone. See *Intent channel*.

## Intent and why

The framework hides agent behavior by design: intent recognition and
prompt generation run headless, out of sight, inside containers. But a
fully hidden agent leaves the user with no standing — they cannot see
what matters and cannot steer where steering is genuinely required.

The Macro UI shows **only what the user wants to see**: the decisions
that need them, and the outputs produced for them. Everything else —
prompts, tool calls, the agent loop, scrolling logs — stays behind the
container wall.

This is a **generic framework** piece, not a Bridge Studio feature. The
skeleton (stage bar + output area + decision bar) is domain-neutral;
each instance injects its own stages, outputs, and decision points. No
domain knowledge lives in the framework layer. Bridge Studio is the
first instance.

## Goals

- The user sees: pipeline stage status at a glance, the current output
  preview, and pending decisions.
- The user acts: decide (lock/unlock, adopt/re-render, retry, restore a
  version), and open a troubleshooting drawer when something needs
  inspection.
- Zero new serving dependencies: the orchestrator already runs an HTTP
  server and holds all state; the UI is another view of that state.
- Generic skeleton, instance-injected content.

## Non-goals

- **No intent input box anywhere.** Intent enters through behavior
  inference only; recognition happens in the hidden agent layer. The
  user never writes text and never learns anything new.
- No exposure of agent internals: no prompts, no tool-call traces, no
  agent loop, no scrolling logs.
- Pipeline performance / efficiency work (explicitly frozen).
- Multi-instance management UI (future work).
- Building the inference layer (record-only this milestone — see
  *Intent channel*).

## Anti-goals

- A control panel that reveals the agent's machinery — that defeats the
  hiding principle.
- A monitoring dashboard that expands intermediate state by default.
  Intermediate state (pose history, snapshots, per-stage detail) is
  log-like: shown only when troubleshooting or making a targeted
  adjustment.
- The TUI form (evaluated and rejected — see Approaches).
- Discarding user work, ever — including on re-render (see *Content
  inventory*).

## Constraints

- Generic framework: no domain knowledge in the framework layer; Bridge
  Studio is an instance that injects content into the skeleton.
- Output is inherently visual (canvas preview, thumbnails) — the chosen
  form must render images reliably.
- macOS reference machine; a browser is available.
- Rendering is deterministic given a pose: re-rendering the same frozen
  pose yields the same image. "Re-render" therefore means returning to
  the adjustment phase to re-orbit, not re-running the same render.
- dev-lines discipline: PR base is `dev`; this line stacks on PR #2
  until it merges, then retargets.

## Approaches

### Form — picked A

- **A · Web GUI served by the orchestrator (picked).** The orchestrator
  already serves HTTP (:8791) and owns all state; the UI is just a view
  of that state — zero new dependencies. Images and thumbnails render
  natively. The skeleton is reusable by any future instance. Cost: a
  browser window. (The brainstorming mockups the user approved were
  themselves served this way — a live demonstration of the form.)
- **B · TUI (rejected).** A terminal UI handles decisions and lists
  well, but output preview is fatal: terminal image protocols
  (kitty / iTerm2 inline / sixel) are fragmented and unsupported by
  macOS Terminal.app; character-mosaic approximations are useless for
  judging a painting; falling back to an external image viewer splits
  the experience in two. It also adds an event loop, key handling,
  layout code, and new dependencies.
- **C · Web primary + one-line TUI fallback for ssh (rejected for
  now).** Two surfaces to maintain; the rarely-used fallback rots;
  over-design at this stage.

### Layout — picked C (refined)

- A: decision bar + output main area.
- B: output tabs + persistent decision strip.
- **C: pipeline overview with per-stage inline decisions (picked),
  refined in conversation:** stage nodes carry only status dots
  (✓ done, ● running, ○ waiting); the main area shows the current
  output; decisions surface in a persistent bar; intermediate state
  collapses into a "🔧 troubleshoot / adjust" drawer, expanded only for
  troubleshooting or targeted adjustment — it is log-like, not a default
  view.

## The decided how

### Content inventory

- **Decision points.**
  - **Lock / unlock** — switch between adjustment and locked phases;
    the only GUI operation window.
  - **Adopt / re-render (user decision).** Re-render is *cumulative*:
    the painter's undo, the writer's revision. It never discards and
    always accumulates. Concretely: re-render returns the session to
    the adjustment phase to re-orbit; the painted `.kra` is archived as
    a *superseded* version (kept, restorable); the next render/paint
    cycle produces a new version in the same lineage. Adopt durably
    records the current version as adopted (sidecar + ledger event)
    and closes the decision. Decisions never block the next lock.
  - **Restore (user decision).** Because going back must always be
    possible, any archived version can be restored — pushed back into
    Krita — reusing the existing push path. Restore creates a new
    version in the lineage rather than overwriting anything.
  - **Failure retry** — retry or abandon a failed container stage.
  - **Offline state** — see below.
- **Outputs.**
  - Current canvas preview (see *Preview freshness*).
  - Archive list: every version in the lineage, with sidecar metadata,
    marked adopted / superseded, with restore.
- **Drawer (collapsed by default):** pose history · snapshots ·
  per-stage detail · failure records.
- **Deliberately absent:** prompts · tool calls · agent loop ·
  scrolling logs.

### Preview freshness (author's call)

The `.kra` on disk is written once at push time and afterwards only
when Krita saves — Krita's API has no per-stroke hooks, and auto-saving
the user's document is forbidden (it would clobber user work and fight
the user's own save habits). So: the adapter periodically exports a
**flattened preview PNG** (read-only against the open document, never
touches the `.kra` on disk) and the UI serves that. Before any
painting, the preview is the pushed render itself. Export cadence is
left to the builder (a few seconds; cheap and incremental).

### Offline states (author's call)

Three, each displayed with its decision:

- **Container pipeline unavailable** — show + retry button.
- **Krita adapter stale** — the adapter heartbeats on its existing
  poll loop; the UI shows staleness. Reconnect is automatic (the
  adapter keeps polling), so there is no button.
- **Pose stream stale** — derived from the existing `pose_age_ms`
  signal.

"Orchestrator offline" cannot be shown by a page the orchestrator
serves and is out of scope.

### Relationship to the Krita docker (user decision)

The web page is the **sole control surface**. The Krita Bridge docker
slims to a pure push agent inside Krita: no lock buttons, no
pose-history UI (the docker pose-history queued in the parent plan is
cancelled). The browser window follows the brainstorming-companion
form: the user's default browser opened on a keyed localhost URL.

This supersedes the parent spec's "the docker is our only UI" and
relaxes its "no use of the user's own browser profile" anti-goal; the
product story moves from "one studio window" to **one painting window +
one decision page**.

### Intent channel (user decision)

Behavior inference only; no text input anywhere in the UI. **This
milestone is record-only:** the ledger records behavior events
(render/re-render counts, lock durations, decision latency, restores)
so a future inference layer has data to learn from. No inferred
behavior changes the system in this milestone.

### Framework split (author's call)

The work is an **additive UI-serving layer** in the orchestrator: the
skeleton/instance separation lives at the serving layer (an instance
manifest describing stages/outputs/decisions + instance static assets),
not in an orchestrator-core refactor. The existing Bridge-shaped core
(`ADJUSTMENT/LOCKED`, pose, session, roster) is untouched in this
milestone; growing the core toward a generic spine is future work.

## Left to the builder

- Route/page structure and JSON response shapes (must preserve the
  skeleton/instance separation described above).
- Version-lineage representation in the archive store (filenames,
  sidecar fields) within the existing "never clobber" rule.
- Preview-export cadence and format details.
- Drawer grouping, and empty/offline presentation details.
- Interaction tech: plain HTML + JS served by the orchestrator (no
  frontend framework) unless the builder finds cause.
- Test approach consistent with the existing `studio/` suite.
