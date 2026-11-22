# Macro UI — show only what the user wants to see

Date: 2026-11-23
Branch: `dev-macro-ui` (stacks on PR #2 `dev-bridge-studio`; retarget to
`dev` once #2 merges)
Status: draft — pending builder check
Builds on: `docs/superpowers/specs/2026-11-22-bridge-studio-design.md`

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
- The user acts: decide (lock/unlock, adopt/re-render, retry), and open
  a troubleshooting drawer when something needs inspection.
- Zero new serving dependencies: the orchestrator already runs an HTTP
  server and holds all state; the UI is another view of that state.
- Generic skeleton, instance-injected content.

## Non-goals

- **No intent input box anywhere.** Intent enters through **behavior
  inference** only (e.g. repeated re-render requests ≈ dissatisfaction
  with the output); recognition happens in the hidden agent layer. The
  user never writes text and never learns anything new.
- No exposure of agent internals: no prompts, no tool-call traces, no
  agent loop, no scrolling logs.
- Pipeline performance / efficiency work (explicitly frozen).
- Multi-instance management UI (future work).

## Anti-goals

- A control panel that reveals the agent's machinery — that defeats the
  hiding principle.
- A monitoring dashboard that expands intermediate state by default.
  Intermediate state (pose history, snapshots, per-stage detail) is
  log-like: shown only when troubleshooting or making a targeted
  adjustment.
- The TUI form (evaluated and rejected — see Approaches).

## Constraints

- Generic framework: no domain knowledge in the framework layer; Bridge
  Studio is an instance that injects content into the skeleton.
- Output is inherently visual (canvas preview, thumbnails) — the chosen
  form must render images reliably.
- macOS reference machine; a browser is available.
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

- **Content inventory.**
  - Decision points: lock/unlock · adopt/re-render · failure retry ·
    offline state.
  - Outputs: current canvas preview (from the `.kra` thumbnail) ·
    archive list (`.kra` + sidecar metadata).
  - Drawer (collapsed by default): pose history · snapshots ·
    per-stage detail · failure records.
  - Deliberately absent: prompts · tool calls · agent loop · scrolling
    logs.
- **Intent channel:** behavior inference only; no text input anywhere
  in the UI.
- **First instance — Bridge Studio:** extend the existing orchestrator
  (:8791) to expose its state as JSON and serve the page. What the
  artist sees: lock → paint → output preview → adopt or re-render.
  Nothing else.

## Left to the builder

- Route/page structure and JSON response shapes (must preserve the
  skeleton/instance separation).
- How the `.kra` thumbnail is extracted and served (check what the
  archive module already provides).
- Drawer grouping, and empty/offline presentation details.
- Interaction tech: plain HTML + JS served by the orchestrator (no
  frontend framework) unless the builder finds cause.
- Test approach consistent with the existing `studio/` suite.
