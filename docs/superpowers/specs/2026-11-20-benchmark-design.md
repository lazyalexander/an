# An layered benchmark design

Date: 2026-11-20
Branch: `dev-benchmark`
Status: proposed

## Intent

Build reproducible demos and benchmarks that show both what An enables and where its time, throughput, and coordination costs go. The first demos are a private MCP bridge flow and a document production pipeline. The benchmark must separate An control-plane overhead, Apple Container execution, artifact movement, and optional model/harness costs.

The first iteration measures and reports; it does not optimize the kernel or change product behavior.

## Goals

- Exercise the shipped CLI and task-scoped MCP gateway rather than a parallel kernel or mocked mount path.
- Provide two visible demos with deterministic inputs and automatic acceptance checks.
- Produce raw per-run records, aggregate statistics, and a readable report with environment and image identities.
- Separate cold/warm, sequential/concurrent, host/model, and successful/failed operations.
- Make permission decisions deterministic without removing permission checks from the measured path.
- Keep all experimental runners, datasets, and observation tooling in a dedicated, gitignored `lab/benchmark/` package that is not pushed by default.

## Non-goals and anti-goals

- No kernel optimization or new product API in the benchmark branch.
- No claim that results on one Mac generalize to other hardware or Apple Container versions.
- No synthetic score presented as a universal measure of Agent intelligence.
- No bypass of admission, task grants, turns, workspace flows, handoffs, publication, or bridge authorization.
- No silent fallback to a mock when Apple Container, an image, a model, or an MCP dependency is unavailable.
- No automatic replay after an uncertain downstream MCP outcome.
- No benchmark artifacts in the product runtime or standard test path.

## Approaches considered

### Dedicated lab runner through real CLI/MCP — selected

A standalone Python runner drives `target/release/an` and real spool images. It uses the standard library where possible and the existing Python MCP client only for MCP-level cases. It records monotonic timestamps around real operations and preserves raw results.

This gives representative end-to-end paths while keeping observation tooling disposable. It requires more initial work than shell scripts and must explicitly manage trusted-host approvals.

### Extend the smoke scripts

This is the shortest route to a few timings, but datasets, repetitions, statistics, and reports would become scattered. The existing smoke scripts remain correctness checks and are not converted into the benchmark framework.

### Product-grade benchmark package immediately

A permanent Rust or Python benchmark interface would be more polished, but it would freeze the workload and result schema before experiments show what is useful. Promotion, if justified later, means clean reimplementation with standard tests rather than moving the lab package into the product.

## Lab boundary

All new executable benchmark material lives under `lab/benchmark/`. The lab is ignored by default, is not a Cargo workspace member, has no dependency from product code, and is not pushed without an explicit decision. The design document is the only tracked repository addition in this stage.

The runner invokes the real release binary and uses isolated `AN_HOME` directories under the system temporary directory. Each case cleans up its instances and temporary state. A failed cleanup is reported rather than hidden.

## Demo workloads

### Demo C: private document exchange

Flow:

```text
local source file
  → trusted MCP receive bridge
  → private artifact
  → transform spool through a named input
  → private candidate
  → trusted MCP send bridge
  → local destination file
```

The source is a deterministic UTF-8 document. The transform performs a deterministic, easily verified operation such as normalized line endings and a declared text transformation. The destination must byte-match an independently computed expected result.

The case verifies that:

- both bridge directions and the named spool input pass through their specific grants and workspace flows;
- the intermediate candidate never appears in workspace deliveries;
- the receiving bridge gets only the authorized revision;
- no public export is required;
- the final bytes and SHA-256 match the expected result.

Primary measurements are bridge import/export latency, spool execution, end-to-end latency, throughput, and approval count.

### Demo A: document production pipeline

Flow:

```text
Markdown source + template
  → renderer spool through named inputs
  → private PDF candidate
  → verifier spool through handoff
  → approved publication
  → local export
```

The renderer produces a PDF from fixed Markdown and template inputs. The verifier checks the PDF structure, expected page count or page-count range, and required extracted text. Acceptance is based on content and validity, not a byte-identical PDF hash.

The case verifies required named inputs, private handoff to a different spool, publication approval, and export approval. It reports the produced files and acceptance details in addition to timings.

## Benchmark suites

### B0: control plane

Measure real CLI and MCP operations without loading a container:

- task creation and state query;
- turn begin/end;
- permission request, decision, and exact retry;
- built-in MCP schema validation and state calls;
- downstream MCP routing without artifact transfer.

Report operation latency and operations/second. Approval decision time is recorded separately from the retried operation's service time.

### B1: container lifecycle

Measure:

- cold load with no existing instance, while retaining the admitted image and Apple image cache; this is instance-cold, not a first-install/cache-cleared measurement;
- repeated load of an already running instance;
- first run and subsequent runs on one instance;
- run stop/start boundary, status polling, cancellation, and unload;
- demo A end-to-end cold and warm execution.

The guest command for the minimal lifecycle case is fixed and does no substantial I/O. This separates lifecycle overhead from document rendering work. Poll interval is recorded and run completion uses the real status transition rather than a fixed sleep.

### B2: artifact data paths

Exercise handoff, publish, export, bridge input, and bridge output independently and in demo C/A.

Payload ladder:

- one file of 4 KiB, 1 MiB, and 32 MiB;
- 1, 100, and 1000 files with 32 MiB total payload to expose per-file costs.

Report elapsed time, effective MiB/second using actual payload bytes, host `AN_HOME` growth, and host process peak RSS where the platform can attribute it reliably. If VM/container memory cannot be attributed reliably, omit it rather than substitute a misleading number.

### B3: concurrency and isolation

Run the minimal no-substantial-I/O container workload and demo C at 1, 2, 4, and 8 independent tasks. Do not replicate every microbenchmark or demo A across the concurrency matrix in the first iteration. Each task has its own instances and permissions; demo C uses separate workspaces to verify bridge and delivery isolation.

Report per-task latency, aggregate completed runs/second, failure count, SQLite busy/lock errors, cleanup failures, and throughput scaling relative to one task. Correctness gates reject cross-task inputs, wrong recipients, and unexpected public deliveries.

Start with a small bounded run count so an 18-core host is not assumed to mean eight Apple Container VMs can run without resource contention.

### B4: end-to-end Agent cost and quality

Run demos A and C through two clearly separated modes:

1. A deterministic scripted controller is mandatory and validates the full protocol without model variance.
2. An optional generic external-harness adapter uses the same workload and acceptance checks when the user supplies configuration and credentials outside the repository. The adapter is configured by an explicit JSON file path and launches a user-configured command. It sends a versioned workload request as JSON on stdin and reads one JSON result on stdout. The request identifies the demo, task inputs, image digests, and An MCP launch configuration; the result reports completion plus optional model timing and usage. Secrets come from the external command's environment or private configuration and are never copied into benchmark records.

For repeated model runs, report task success, acceptance score, end-to-end latency, model latency, input/output tokens when supplied by the harness, number of tool calls, permission requests, automatic approvals, human approvals, and human waiting time. Missing model usage fields remain null, never zero.

Model comparisons require a fixed model/configuration label, prompt/workload version, image digests, and repetition count. Failures are classified as protocol, permission, container, artifact, acceptance, model, or harness errors.

### B5: fault and recovery behavior

This is primarily a correctness suite, not a performance leaderboard. Inject:

- container exit during a run;
- bridge timeout and late completion;
- permission policy rotation while a request is pending;
- gateway/controller interruption and reconnect;
- worker launch failure and cleanup retry.

Verify terminal or recoverable state, ticket/grant revocation, absence of cross-task leakage, no duplicate external effect after uncertain outcomes, and successful cleanup. Recovery time is secondary and labeled separately.

## Measurement protocol

- Use a release build with locked Cargo dependencies.
- Record git commit and dirty state, full binary path, macOS build, architecture, CPU count, memory, Apple Container version when available, image/platform digests, workload version, and command arguments without credentials.
- Use a monotonic clock for durations and UTC timestamps only for run identity.
- Define end-to-end latency from the initial operation submission to verified final output. Define service latency separately, excluding intentionally injected human approval delay.
- Warm up each suite once per worker/configuration. Default measured repetitions: 20 for B0, 5 for B1, 3 for B2/B3, and 5 for deterministic demo runs. Model repetitions are configurable and explicit.
- Report sample count, successes, failures, median, p95, minimum, and maximum. Do not report p99 for the default small samples. Show raw samples alongside aggregates.
- Run configurations sequentially by default; only B3 intentionally overlaps tasks. Do not compare a cold sample with a warm median.
- Treat acceptance or cleanup failure as a failed sample; never silently discard it from aggregates.
- Keep payload generation deterministic and independent of the operation being timed.

## Result contract

Each invocation creates a run directory outside the product source tree containing:

- `environment.json`: machine, runtime, binary, workload, and image identities;
- `samples.jsonl`: one record per operation/sample with labels, timestamps, bytes, status, and error category;
- `summary.json`: grouped aggregates and correctness results;
- `report.md`: concise human-readable tables and demo output locations;
- demo outputs needed for inspection.

The schema carries a version. Summary calculations use only the standard library. Reports state skipped suites and the reason; skipped is not passed.

## Implementation slices after approval

1. Add the gitignored lab boundary and common runner, environment capture, approval helper, result schema, and cleanup handling.
2. Build and admit the deterministic spool images and local bridge peer; implement demo C with acceptance checks.
3. Implement demo A and its renderer/verifier acceptance checks.
4. Add B0/B1/B2 measurements and raw/summary reporting.
5. Add B3 concurrency and B5 fault cases.
6. Add the optional real-harness adapter and B4 model cost reporting; the scripted controller remains the required baseline.
7. Run the demos and suites on the reference machine, inspect raw records, and publish the baseline report inside the lab only.

## Reference environment for the first baseline

Measured locally on 2026-11-20; the user may correct these values:

- macOS 27.2.1, build 26B131; arm64.
- 18 logical CPUs reported by `hw.ncpu`; 68,719,476,736 bytes of memory.
- rustc/cargo/clippy 1.98.0; git 2.53.0.
- Xcode developer directory `/Applications/Xcode.app/Contents/Developer`; license check produced no error.
- No `rust-toolchain*`, `config/model.json`, or `clippy.toml` in this repository.
- Single binary Cargo package, not a Cargo workspace.

## Builder decisions

The builder may choose the lab's internal module layout, pinned base images and in-image document tools, exact deterministic fixture contents, command-line flags, and result schema field names. These choices must preserve the workload and measurement contracts above and must not add product dependencies.
