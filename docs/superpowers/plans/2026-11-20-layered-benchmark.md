# Layered Benchmark Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a disposable lab that runs demos A/C through the real An CLI/MCP and reports layered control-plane, container, artifact, concurrency, recovery, and external-harness measurements.

**Architecture:** A Python standard-library runner owns isolated `AN_HOME` state, trusted-host approvals, workload fixtures, raw samples, and aggregate reports. Real spool images and a real stdio MCP peer exercise the shipped release binary; unit tests mock only external process boundaries, while explicitly invoked integration suites run the real paths.

**Tech Stack:** Python 3 `unittest`, `asyncio`, `subprocess`, `sqlite3`, JSON/JSONL; existing Python MCP client; Rust/An release binary; Apple Container; OCI spool images.

**Spec:** `docs/superpowers/specs/2026-11-20-benchmark-design.md`

## Global Constraints

- All executable benchmark material lives in `lab/benchmark/`; product code never depends on it, and the directory is gitignored and not pushed by default.
- Measure `target/release/an` built with `cargo build --release --locked`; never substitute the Python reference kernel for integration measurements.
- Do not bypass admission, task grants, turns, workspace flows, handoffs, publication, or bridge authorization.
- Cold means no existing instance while retaining admitted images and Apple image caches.
- Default repetitions: B0=20, B1=5, B2/B3=3, deterministic demos=5; one warmup per worker/configuration.
- Report raw samples, successes, failures, median, p95, minimum, and maximum; no p99 for default samples.
- B3 covers only the minimal no-substantial-I/O workload and demo C at 1/2/4/8 tasks.
- Real-model access uses an explicit generic external-harness JSON adapter; credentials never enter result records.
- Acceptance, skipped, and cleanup states are distinct; failed samples are never discarded.

## Review Focus

- Malformed external-harness JSON or nonzero exits must become harness failures, not successful samples (Task 8 test).
- A 32 MiB bridge payload is valid, while 32 MiB + 1 byte is rejected without corrupting the result set (Task 5 test).
- A failed sample or cleanup failure must remain visible in `samples.jsonl` and aggregates (Task 2 test).
- Concurrent tasks must never consume another task's candidate or see its private delivery (Task 6 integration assertions).
- Missing Apple Container, images, MCP support, or model configuration must report skipped/error explicitly rather than silently passing (Tasks 3, 4, 7, and 8 tests).

---

### Task 1: Lab boundary and command-line skeleton

**Files:**
- Modify: `.gitignore`
- Create: `lab/benchmark/run.py`
- Create: `lab/benchmark/bench/__init__.py`
- Create: `lab/benchmark/bench/cli.py`
- Test: `lab/benchmark/tests/test_cli.py`

**Interfaces:**
- Produces: `main(argv: list[str]) -> int`; subcommands `demo-c`, `demo-a`, `suite`, and `external`.
- Produces: global options `--an PATH`, `--output PATH`, `--repeat N`, and `--suite NAME` with suite choices `preflight`, `b0`, `b1`, `b2`, `b3`, `b4`, and `b5`; `external` requires `--config PATH`.

- [ ] **Step 1: Write failing CLI parser tests** asserting all four subcommands parse, `--repeat 0` is rejected, unknown suites exit 2, and unimplemented real execution is not attempted by parser tests.
- [ ] **Step 2: Run `python3 -m unittest discover -s lab/benchmark/tests -v`** and verify failure because `bench.cli` does not exist.
- [ ] **Step 3: Implement the parser and dispatch table** with standard-library `argparse`; add `/lab/` to `.gitignore` without changing Cargo membership or product files.
- [ ] **Step 4: Re-run the unit test command** and verify parser tests pass.
- [ ] **Step 5: Commit the tracked design, plan, and `.gitignore` only** with `git add .gitignore docs/superpowers && git commit -m "docs: design layered benchmarks"`; do not force-add the lab.

### Task 2: Real CLI driver and versioned result records

**Files:**
- Create: `lab/benchmark/bench/an.py`
- Create: `lab/benchmark/bench/results.py`
- Test: `lab/benchmark/tests/test_an.py`
- Test: `lab/benchmark/tests/test_results.py`

**Interfaces:**
- Produces: `An.binary`, `An.home`, `An.call(*args: str, turn: str | None = None) -> dict`, and `An.output(*args: str, turn: str | None = None) -> str`.
- Produces: `Sample(suite, operation, labels, start_ns, end_ns, status, bytes=0, error=None)` with `to_dict() -> dict`.
- Produces: `ResultWriter.write_sample(Sample) -> None`, `ResultWriter.environment(dict) -> None`, and `ResultWriter.finish() -> None`.
- Produces: `summarize(samples: list[Sample]) -> dict` and `write_report(run_dir: Path, summary: dict) -> None`.

- [ ] **Step 1: Write failing driver tests** for isolated `AN_HOME`/`AN_TURN`, JSON parsing, nonzero stderr classification, and monotonic timing with a mocked `subprocess.run`.
- [ ] **Step 2: Write failing result tests** for schema version, JSONL append, null optional metrics, median/p95, retained failure/cleanup samples, and explicit skipped output.
- [ ] **Step 3: Run the lab unit suite** and verify the new tests fail on missing modules.
- [ ] **Step 4: Implement the driver and result contract** using `time.monotonic_ns`, UTC run IDs, atomic environment/summary writes, and standard-library statistics; sanitize environment and arguments against credential keys.
- [ ] **Step 5: Add the malformed record and failed-cleanup cases** to prove neither crashes nor disappears from aggregation.
- [ ] **Step 6: Run the lab unit suite** and verify all tests pass.
- [ ] **Step 7: Commit only deliberate tracked changes if any**; normally this task remains entirely inside the ignored lab.

### Task 3: Trusted host workflow and deterministic images

**Files:**
- Create: `lab/benchmark/bench/host.py`
- Create: `lab/benchmark/bench/images.py`
- Create: `lab/benchmark/peers/file_bridge.py`
- Create: `lab/benchmark/spools/text-transform/Dockerfile`
- Create: `lab/benchmark/spools/text-transform/spool.yaml`
- Create: `lab/benchmark/spools/text-transform/SKILL.md`
- Create: `lab/benchmark/spools/text-transform/code/transform`
- Create: `lab/benchmark/spools/document-renderer/{Dockerfile,spool.yaml,SKILL.md,code/render}`
- Create: `lab/benchmark/spools/document-verifier/{Dockerfile,spool.yaml,SKILL.md,code/verify}`
- Test: `lab/benchmark/tests/test_host.py`

**Interfaces:**
- Consumes: `An` from Task 2.
- Produces: `Host.new_workspace(images: list[str], tools: list[str], flows: list[list[str]]) -> Host`, `Host.begin() -> None`, `Host.approve(call: Callable[[], dict]) -> dict`, and `Host.cleanup() -> None`.
- Produces: `build_and_register(an: An, definitions: list[Path]) -> dict[str, str]` returning digest-pinned image references.
- Produces: bridge tools `produce(text: str, an_transfer: dict)`, `consume(an_transfer: dict)`, and `send(an_transfer: dict)`.

- [ ] **Step 1: Write failing host tests** proving pending requests are decided once and retried with the same request ID, denied/unknown results are not retried, and cleanup runs after errors.
- [ ] **Step 2: Run the host tests** and verify failure on missing `bench.host`.
- [ ] **Step 3: Implement the trusted-host helper and preflight errors**; missing release binary, `container`, Python MCP, or image build failure must raise a labeled prerequisite error.
- [ ] **Step 4: Implement deterministic spool definitions and the real FastMCP peer** with concrete manifest ports, nonempty code, `/bin/sleep infinity`, no network requirement, and no host credential mounts.
- [ ] **Step 5: Run unit tests plus `python3 lab/benchmark/run.py suite --suite preflight`**; unit tests must pass and preflight must either report all prerequisites or a specific missing one.
- [ ] **Step 6: Run `cargo build --release --locked` and build/register the images** only when Apple Container is available; verify `an state` reports each image admitted.

### Task 4: Demo C private bridge with acceptance

**Files:**
- Create: `lab/benchmark/bench/demo_c.py`
- Test: `lab/benchmark/tests/test_demo_c.py`

**Interfaces:**
- Consumes: `An`, `Host`, `ResultWriter`, `build_and_register`, and the file bridge peer.
- Produces: `run_demo_c(an: An, writer: ResultWriter, repeat: int = 5) -> dict` and `accept_demo_c(output: Path, expected: Path) -> dict` for reuse by the external adapter.

- [ ] **Step 1: Write failing orchestration tests** with fake host/transport boundaries asserting the exact flow `receive artifact → named transform input → private candidate → bridge send`, expected-byte validation, and no published delivery.
- [ ] **Step 2: Add a prerequisite-failure test** asserting an unavailable bridge marks the run skipped/error rather than passed.
- [ ] **Step 3: Run demo C unit tests** and verify failure on the missing module.
- [ ] **Step 4: Implement `run_demo_c`** with one warmup, measured samples for each stage and end-to-end operation, deterministic source/expected bytes, SHA-256 checks, and guaranteed instance cleanup.
- [ ] **Step 5: Run unit tests, then `python3 lab/benchmark/run.py demo-c --repeat 1`** against real images; verify destination bytes, absent public delivery, valid JSONL, and a non-passing report on any failed acceptance check.

### Task 5: Demo A and B0/B1/B2 core suites

**Files:**
- Create: `lab/benchmark/bench/demo_a.py`
- Create: `lab/benchmark/bench/suites.py`
- Test: `lab/benchmark/tests/test_demo_a.py`
- Test: `lab/benchmark/tests/test_suites.py`

**Interfaces:**
- Produces: `run_demo_a(an: An, writer: ResultWriter, repeat: int = 5) -> dict` and `accept_demo_a(document: Path, required_text: list[str], page_range: tuple[int, int]) -> dict` for reuse by the external adapter.
- Produces: `run_suite(name: str, an: An, writer: ResultWriter, repeat: int | None = None) -> dict` for `b0`, `b1`, and `b2`.
- Produces: payload cases `(4 KiB, 1 file)`, `(1 MiB, 1 file)`, `(32 MiB, 1 file)`, and `(32 MiB, 100 or 1000 files)`.

- [ ] **Step 1: Write failing demo A tests** for required named inputs, private renderer-to-verifier handoff, separate publish/export approvals, PDF validity/text/page acceptance, and no byte-hash equality assumption.
- [ ] **Step 2: Write failing suite tests** for default repetition counts, cold instance reuse rules, monotonic status polling, payload ladders, `AN_HOME` growth, throughput formulas, and 32 MiB + 1 rejection.
- [ ] **Step 3: Run these unit tests** and verify failure on missing implementations.
- [ ] **Step 4: Implement demo A and the three suites** using the prior host/result interfaces; record stage service time separately from approval and end-to-end time, and omit unattributable VM memory.
- [ ] **Step 5: Run the full lab unit suite** and verify all tests pass.
- [ ] **Step 6: Run `demo-a --repeat 1`, then `suite --suite b0`, `b1`, and `b2 --repeat 1`**; inspect raw records and generated Markdown rather than trusting only exit status.

### Task 6: B3 concurrency and isolation

**Files:**
- Create: `lab/benchmark/bench/concurrent.py`
- Test: `lab/benchmark/tests/test_concurrent.py`

**Interfaces:**
- Produces: `run_b3(an: An, writer: ResultWriter, levels: list[int] = [1, 2, 4, 8], repeat: int = 3) -> dict`.

- [ ] **Step 1: Write failing scheduling tests** for bounded 1/2/4/8 fan-out, one warmup per worker, retained individual failures, aggregate throughput, and scaling calculations.
- [ ] **Step 2: Write failing isolation assertions** for wrong-task candidate rejection, wrong-recipient handoff rejection, and private delivery invisibility.
- [ ] **Step 3: Run tests and verify failure**, then implement thread-based orchestration around independent subprocess CLI calls and separate task/workspace state; do not share an `AN_TURN` between tasks.
- [ ] **Step 4: Run unit tests and real `suite --suite b3 --repeat 1`**, starting at level 1 before the full matrix; verify no lock error is hidden and every task cleans up.

### Task 7: B5 fault injection and recovery

**Files:**
- Create: `lab/benchmark/bench/faults.py`
- Test: `lab/benchmark/tests/test_faults.py`

**Interfaces:**
- Produces: `run_b5(an: An, writer: ResultWriter, cases: list[str] | None = None) -> dict` with named cases `container-exit`, `bridge-timeout`, `policy-rotation`, `reconnect`, and `worker-launch-failure`.

- [ ] **Step 1: Write one failing correctness test per case** for terminal/recoverable state, revocation, cleanup, and the no-automatic-replay rule after uncertain bridge outcomes.
- [ ] **Step 2: Add a missing-prerequisite test** proving unsupported process interruption is reported skipped rather than passed.
- [ ] **Step 3: Run tests and verify failure**, then implement explicit fault triggers and independent postconditions without changing product code.
- [ ] **Step 4: Run unit tests and `suite --suite b5`**; inspect every case's postcondition and cleanup record, keeping recovery timing secondary to correctness.

### Task 8: Generic external-harness adapter and B4 reporting

**Files:**
- Create: `lab/benchmark/bench/external.py`
- Create: `lab/benchmark/tests/test_external.py`

**Interfaces:**
- Produces: `run_external(an: An, writer: ResultWriter, config: Path, repeat: int) -> dict`.
- Config: `{version: 1, command: list[str], demo: "demo-c" | "demo-a", model: str, configuration: dict}`.
- Protocol: request JSON on stdin; one result JSON on stdout with `status`, optional `model_latency_ms`, `input_tokens`, `output_tokens`, `tool_calls`, and harness diagnostics.

- [ ] **Step 1: Write failing protocol tests** for request version/content, null usage preservation, timeout, nonzero exit, malformed JSON, wrong schema version, and secret redaction.
- [ ] **Step 2: Run tests and verify failure**, then implement the adapter with explicit config path, subprocess environment inheritance without secret logging, and the same demo acceptance functions used by deterministic runs.
- [ ] **Step 3: Run unit tests and an external fake-harness contract run**; verify usage fields and failures in raw output. Do not contact a real model without user-supplied configuration.
- [ ] **Step 4: Run `python3 lab/benchmark/run.py external --config ... --repeat 1`** only with user-approved real configuration, otherwise record the real-model suite as explicitly skipped.

### Task 9: Baseline validation and handoff

**Files:**
- No product files; each generated baseline remains in its `--output` directory outside the repository, defaulting to a unique directory under the system temporary directory.

**Interfaces:**
- Consumes all prior commands and result contracts.

- [ ] **Step 1: Run `python3 -m unittest discover -s lab/benchmark/tests -v`** and require all lab unit tests to pass.
- [ ] **Step 2: Run `cargo test --locked`, `cargo clippy --all-targets -- -D warnings`, and `cargo fmt --all -- --check`** to prove the tracked product change is limited and healthy.
- [ ] **Step 3: Run both demos with `--repeat 1`, then deterministic demos with default repeat and B0/B1/B2**; inspect `environment.json`, `samples.jsonl`, `summary.json`, `report.md`, PDF output, and bridge output.
- [ ] **Step 4: Run B3 and B5**, then B4 with the fake harness; run a real model only when separately configured and approved.
- [ ] **Step 5: Check `git status --short --branch`**; only intended tracked docs/.gitignore changes may appear, with no generated state or lab files force-added. Report baseline limits, failures, skipped suites, and machine-specific results without claiming cross-machine generality.
