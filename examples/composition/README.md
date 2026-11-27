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

To replay the flat-colour answer from a planning trial, add `--plan /tmp/an-plan-001/flat-colour/plan.json` to the relay command and use a fresh output directory. The host receives the recorded nodes with catalog names resolved to registered image digests.

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

## Change the goal while delivery is waiting

Build An and run the Studio acceptance on a Mac with Apple Container running and a signed-in Codex CLI:

```sh
cargo build --locked
python3.13 examples/composition/studio_turns.py --out "$(mktemp -d)/result"
```

The fixture attaches pen strokes to a fresh task. It sends both instructions through the Studio HTTP API. Codex selects the spools, proposes the routes and submits each graph revision through delegated MCP. The test acts as the operator and reviews each route against the fixture. It holds the first line-art publication until Codex has received the second instruction and extended the same graph. The second instruction asks for a flat-colour painting of that exact line art and a separate copy of each result.

A passing report records the pending publication and producer attempt at the point the user changed the goal, both submission cursors, the Codex thread and traces, both delivered hashes, and cleanup of the task's containers. Files under the run directory may contain active harness credentials. Use the summary `report.json` to share results.

A separate [paused planning run](baselines/paused-codex-replan.json) used the Rust CLI to send the two instructions and a real Codex session over stdio MCP. It supplied the public spool skills from a previous admission. The host accepted revisions 1 and 2. The second added only painting and its delivery, and its input referenced the original line-art node. The graph stayed paused throughout this run.

## Large binary artifacts on the Rust host

A single input revision feeds two delivery nodes. Each publishes the binary under a different path, as happens when two results share a file. The benchmark starts a fresh registry per trial, approves the publish route, authorizes each export individually, and checks both exported files against the original SHA-256. It uses the production Rust CLI. Python 3.9+ and macOS or Linux are sufficient.

```sh
cargo build --release --locked
python3 examples/composition/artifact_baseline.py > /tmp/an-artifacts.json
```

Release build on macOS 27.2.1 arm64, three trials per size. Each timing includes CLI startup. RSS is the maximum resident memory of that CLI process. Import and publish columns show the median of three samples and export shows the median of six. Registry size is the logical file size after each operation. No OS cache flush was performed. The script accepts `--sizes-mib`, `--trials` and `--an`. The [raw samples](baselines/artifacts-macos-arm64.json) include the binary checksum.

| Input (MiB) | Import ms / RSS MiB | Two deliveries ms / RSS MiB | One approved export ms / RSS MiB | Registry after import / publish (MiB) |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 11.8 / 14.9 | 18.4 / 15.0 | 5.6 / 9.7 | 1.34 / 4.03 |
| 16 | 103.5 / 111.1 | 195.6 / 117.1 | 18.5 / 50.4 | 21.36 / 64.09 |
| 64 | 383.2 / 415.1 | 717.9 / 437.1 | 55.5 / 178.5 | 85.43 / 256.27 |

All 18 authorized exports matched their input. At 64 MiB, the registry retained about four times the input size. The host stores base64 in revision JSON. Prefixing two distinct delivery paths produces two more revision bodies containing the same bytes. Publishing used about 6.8 times the input size in peak RSS. The measurements cover task import, graph publication and local export. Guest staging, output capture and bridge transfer have separate paths. Apple Container returned `Operation not permitted` when queried from this benchmark environment, so guest costs remain unmeasured.
