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
| `agent_message_max_bytes` | 4096 | Size of one agent message to the person |
| `image_port` | 11435 | Loopback port of the local image service |
| `image_timeout_s` | 600 | One image generation |
| `image_output_bytes` | 33554432 | Size of an image input or result |
| `model_aliases` | `{"local":"qwen3:8b"}` | Trusted alias mapping to installed local Ollama models |
| `model_default_calls` | 4 | Task quota when the host grants an alias |
| `ollama_port` | 11434 | Ollama port on 127.0.0.1 |
| `model_timeout_s` | 120 | HTTP request timeout |
| `model_input_bytes` | 16384 | Maximum frozen request size |
| `model_output_tokens` | 256 | Generation limit supplied to Ollama |

Format contracts that admission depends on stay in code: the 64 KiB `spool.yaml` limit, export path budgets and digest shape. An image is therefore admitted identically on every host.

## Studio

[`studio/config/studio.json`](../studio/config/studio.json) selects the locale and holds Studio settings (`graph.poll_ms`, `graph.panel_timeout_ms`, `graph.max_body_bytes`). The `agent` section configures the [agent runner](graph-studio.md#agent-runner): `command` (null disables it), `poll_ms`, `timeout_s` per wake, and `env`, the extra variable names passed to the agent. Pass `--config FILE` to `python3 -m orchestrator` to override them; the same unknown-key rule applies.

Task Studio reads its labels from `studio/config/locales/<locale>.json`. The legacy experiment owns its instance manifests and translations, including Krita panel text, under `examples/legacy_studio/config/`.

`model_aliases` maps an alias to a local Ollama model name, or to an image backend object: `{"kind": "image", "backend": "local", "model": ...}` or `{"kind": "image", "backend": "openai", "model": ..., "keychain": SERVICE}`. API keys are never configuration; they live in the Keychain entry named by `keychain`.


Studio and the headless controller accept JSON or TOML overrides through `--config`. They apply the same validation to both formats. For example:

```toml
[agent]
poll_ms = 500
timeout_s = 900
```

The repository-owned `studio/an_host/controller.toml` declares the graph operations the presentation can forward and the import boundary checked by the Studio tests.
