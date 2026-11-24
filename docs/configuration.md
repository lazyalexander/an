# Configuration

Settings and user-facing text are configuration, not code.

## An host

Operational defaults live in [`config/an.json`](../config/an.json) and are compiled into the binary. A host overrides any of them in `$AN_HOME/config.json`. Unknown keys are rejected. Numeric settings require positive integers.

| Setting | Default | Meaning |
| --- | --- | --- |
| `sqlite_busy_s` | 20 | Registry lock wait |
| `mcp_deadline_s` | 30 | Downstream MCP startup, discovery and ordinary calls |
| `mcp_ping_s` | 3 | Downstream liveness check in `an_state` |
| `bridge_deadline_s` | 240 | Artifact bridge calls |
| `transfer_limit_bytes` | 33554432 | Bytes a bridge may return per call |
| `graph_expansion_budget` | 256 | Nodes drivers may add to one task |
| `instruction_max_bytes` | 4096 | Size of one relayed instruction |
| `model_aliases` | `{"local":"qwen3:8b"}` | Trusted alias mapping to installed local Ollama models |
| `model_default_calls` | 4 | Task quota when the host grants an alias |
| `ollama_port` | 11434 | Ollama port on 127.0.0.1 |
| `model_timeout_s` | 120 | HTTP request timeout |
| `model_input_bytes` | 16384 | Maximum frozen request size |
| `model_output_tokens` | 256 | Generation limit supplied to Ollama |

Format contracts that admission depends on stay in code: the 64 KiB `spool.yaml` limit, export path budgets and digest shape. An image is therefore admitted identically on every host.

## Studio

[`studio/config/studio.json`](../studio/config/studio.json) selects the locale and holds Studio settings (`graph.poll_ms`, `graph.panel_timeout_ms`, `graph.max_body_bytes`). Pass `--config FILE` to `python3 -m orchestrator` to override them; the same unknown-key rule applies.

Every string shown by the task page, the legacy macro UI and the Krita docker lives in `studio/config/locales/<locale>.json`. Instance manifests, including their labels, live in `studio/config/instances/<name>.json`. Tests reject CJK text in Studio code and check that every text key the pages and docker use exists in the locale.
