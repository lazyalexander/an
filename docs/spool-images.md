# Spool image requirements

An v0.3 uses `/bin/sleep infinity` as the idle process for explicitly loaded instances. The image must supply this executable and support that argument. Registration tests the pinned image in a disposable, network-isolated container with a read-only root filesystem. A failed readiness check prevents registration.

Each guest command ends at a VM stop boundary. An confirms the container has stopped and restarts its idle process before publishing results or accepting another command. The instance ID and mounted data persist across commands; guest processes and in-memory services end at the boundary. This adds a container stop/start to each command.

## spool/v2 image admission

Ship the domain instructions as a nonblank, regular UTF-8 file at `/spool/SKILL.md` (maximum 256 KiB). Put at least one nonempty regular file under the real directory `/spool/code/`. Programs and dependencies may use any language. Describe commands, inputs and declared outputs in the skill. A writable task may copy the generator into `/work`; the packaged baseline is read-only.

Registration checks the native linux/arm64 variant in an isolated disposable container, then streams the stopped filesystem export on the host. The admission record pins the image digest, platform variant digest, contract version and skill digest. Re-register legacy images before loading or dispatching new commands. Task state lists authorized image admission summaries, and the task-scoped MCP tool `an_skill(spool)` reads the admitted instructions before load. A successful admission describes structure and runtime compatibility; domain acceptance belongs to the spool author and agent.

Probe and task containers explicitly select the admitted platform. Unload and reload instances created before platform pinning. Export paths may use up to 4096 bytes each. The scanner accepts up to 16,384 entries under /spool, including the directory itself, with a combined path budget of 4 MiB.

### Interface manifest

Package a regular UTF-8 file at `/spool/spool.yaml`. Run `an check spool.yaml` before building. Build the native CLI with `cargo build --release --locked`. The check prints the spool name on success and reports invalid fields or YAML syntax on stderr.

```yaml
schema: an.spool/v1
name: document-renderer
summary: Render a Markdown document as a PDF.
inputs:
  - name: source
    description: Source document
    media_types: [text/markdown]
    required: true
outputs:
  - name: document
    description: Rendered document
    media_types: [application/pdf]
```

All shown keys are required. Either port list may be empty. Names start with a lowercase ASCII letter and contain up to 64 lowercase letters, digits or hyphens. Port names must be unique within each direction. Supply nonblank descriptions and a nonempty list of distinct concrete media types without parameters. These declarations help the agent choose a spool. Runtime authorization continues to use workspace and harness policies.

An reads one YAML document, at most 64 KiB, with at most 16 nested collections and 4096 nodes. Duplicate and unknown keys, anchors, aliases, merge keys and explicit tags are rejected. Registration checks the actual image export and binds the manifest digest to the admitted image and platform variant. Existing spool/v1 admissions need re-registration. Rebuild images missing the manifest before registering their new digest. Task state exposes the name and summary; an_skill returns the full admitted interface.

### Declaring panels with an.spool/v2

Use the v1 interface fields and add a `panels` map (which can be empty). Each entry declares a title, kind, required binding names and optionally an `optional` list of additional binding names. These are the panel's artifact slots, independent of guest command ports. An optional `html` path refers to a self-contained regular UTF-8 document under `spool/code/panels/` in the image. The file must fit the admitted code snapshot (1 MiB per file, 8 MiB in total). Existing v1 manifests remain supported.

```yaml
panels:
  timing:
    title: Rotation period in seconds
    kind: surface
    bindings: []
    decision: {type: adjust, field: seconds, value_type: integer, options: [1, 2, 3, 4, 5], default: 3}
```

| Decision | Required response |
| --- | --- |
| `choose`, `compare` | `choice` names one of at least two required artifact bindings |
| `point` | Normalized `x` and `y` on a bound artifact |
| `region` | Normalized `x`, `y`, `width`, `height` on a bound artifact |
| `adjust` | The named `field` of `value_type` (integer, number or string), optionally constrained by `options` and initialized with `default` |
| `confirm` | Boolean `confirmed` |

Use `decision.extra` to add required text or coordinate fields. Every surface, including a custom HTML one, requires a decision. A delivery panel declares `kind: delivery` and only presents the bound published files. See the full Bridge and CAD declarations in `examples/graph_studio/spool/spool.yaml`.
