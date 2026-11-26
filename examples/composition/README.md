# Catalog composition experiment

Given a sentence and the inputs available to a task, can Codex choose the required tools, connect their artifacts, and deliver the requested results? The five fixture spools implement pen strokes to SVG, line cleanup, flat colour, Markdown TODO review, and XY projection of an OBJ wireframe. They use deterministic file transformations so the harness can verify the delivered bytes. Painting here means flat colour applied to inked lines.

## Graph generation

The normal agent starts with the desired deliverables and calls `an_catalog` to see the task's input names, admitted interfaces, delivery destination and model grant. The catalog reads the admission snapshot. Discovery never starts task instances. Media types narrow the candidates; both rough and inked drawings can be SVG, so the planner must also consider their descriptions and the person's request. The selected spool's skill supplies its command, input slot and concrete output filename.

The agent submits the work it can justify against the current graph revision. Each input references a named task input or a specific producer node and output port. Two consumers may bind the same immutable revision. Independent branches can proceed separately. If later choices depend on a person or an intermediate result, the graph can grow in a subsequent submission. The benchmark measures initial composition with fully specified inputs. Interactive replanning is a separate measurement.

## Instance ownership

| Moment | Host behavior |
| --- | --- |
| Discover and plan | No task containers. For the benchmark, pause the task before giving the planner access. An accepted graph can be inspected with all its nodes queued. |
| Run becomes ready | Resolve input revisions. The scheduler chooses or loads a task-scoped instance. Input authorization can leave the instance reserved while a decision is pending. |
| Execute | A command owns one run attempt. Eligible ordinary instances can be reused for the same task and spool. Parallel read-only runs can receive separate instances. Writable instances serialize access. An idle ordinary instance stays available until explicit unload or task finish. This experiment adds no idle timeout or concurrency budget. Expand drivers have dedicated attempt-owned instances. |
| Hand off | The host captures an immutable output revision. Downstream bindings reference it even after the producer's container has gone away. |
| Pause or stop | Pause starts no new work and allows in-flight work to settle. Stop records cancellation before reconciling runs and driver instances. |
| Finish | Unresolved runs or active instances prevent closure. The host records unload intent for remaining instances in the same transaction that closes the task. Finish returns residual instance IDs in `cleanup_pending`. Repeating finish retries reconciliation; delivery records remain accessible. |

Image registration can use disposable probe containers. Registered images and their build cache have a different lifetime from a task's instances. Fixture tags incorporate the build context so reruns with the same assets reuse the tag. Live reports record the image IDs used.

## Run

From the repository root, with Python 3.11+ and an authenticated Codex CLI:

```sh
python3 examples/composition/evaluate.py --out /tmp/an-plan-001
```

The inexpensive planning pass shows a shuffled snapshot of all five fixture interfaces and their skills, the input filenames, and one sentence at a time. Codex runs in a fresh directory with tools restricted. The evaluator retains the prompt, raw answer, parsed graph and report. Use a new output path for each run; set `--seed`, `--model` or `--case` to reproduce a condition. The fixtures and oracle stay outside the prompt.

To check the runtime independently on an Apple Container host, run the fixed sketch → line art → painting graph. The delivery consumes both the line art and the painting. The check verifies revision bindings, file contents and removal of the task's labelled containers. It records the observed graph on successful execution and writes a separate runtime report. This run needs no Codex installation or login and contributes no planner scores.

```sh
cargo build --locked
python3 examples/composition/relay.py --out /tmp/an-relay-001
```

For execution with Codex on an Apple Container host:

```sh
cargo build --locked
python3 examples/composition/evaluate.py --execute --case parallel --out /tmp/an-live-001
```

The live pass registers five independently built images in a fresh An registry. For each task Codex receives the sentence and uses the real MCP catalog, skill and graph tools. The evaluator reads the submitted graph from the host. It resumes only a graph whose selected spool commands, bindings and requested deliverables pass the fixture oracle. It authorizes those specific routes in this synthetic workspace. It checks the actual upstream and consumer revision IDs, opens the published files, compares their bytes with the deterministic reference results, then finishes the task and checks its instances. A failed graph is stopped before cleanup. The two passes are independent Codex trials; their inputs and tool access differ.

## Interpret the score

Seven cases include distracting inputs, two independent branches, delivery of both an intermediate and its derivative, and an unavailable audio capability. The oracle in `cases.toml` is used only for grading and reference artifacts. Scoring ignores node IDs and list order. It reports exact spool selection, valid command and dependency wiring, and correct delivery bindings separately. Live execution and cleanup have their own outcomes. No accuracy can be inferred from zero scored cases. Provider or runtime failures are recorded as infrastructure errors.

```sh
python3 tests/test_composition.py
```

These regressions use known graphs and synthetic admission records to exercise the scorer and paused Rust/MCP path. They do not measure Codex selection or guest execution.
