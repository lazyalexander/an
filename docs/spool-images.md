# Spool image requirements

An v0.3 uses `/bin/sleep infinity` as the idle process for explicitly loaded instances. The image must supply this executable and support that argument. Registration tests the pinned image in a disposable, network-isolated container with a read-only root filesystem. A failed readiness check prevents registration.

Each guest command ends at a VM stop boundary. An confirms the container has stopped and restarts its idle process before publishing results or accepting another command. The instance ID and mounted data persist across commands; guest processes and in-memory services end at the boundary. This adds a container stop/start to each command.

## spool/v2 image admission

Ship the domain instructions as a nonblank, regular UTF-8 file at `/spool/SKILL.md` (maximum 256 KiB). Put at least one nonempty regular file under the real directory `/spool/code/`. Programs and dependencies may use any language. Describe commands, inputs and declared outputs in the skill. A writable task may copy the generator into `/work`; the packaged baseline is read-only.

Registration checks the native linux/arm64 variant in an isolated disposable container, then streams the stopped filesystem export on the host. The admission record pins the image digest, platform variant digest, contract version and skill digest. Re-register legacy images before loading or dispatching new commands. Task state lists authorized image admission summaries, and the task-scoped MCP tool `an_skill(spool)` reads the admitted instructions before load. A successful admission describes structure and runtime compatibility; domain acceptance belongs to the spool author and agent.

Probe and task containers explicitly select the admitted platform. Unload and reload instances created before platform pinning. Export paths may use up to 4096 bytes each. The scanner accepts up to 16,384 entries under /spool, including the directory itself, with a combined path budget of 4 MiB.

### Interface manifest

Package a regular UTF-8 file at `/spool/spool.yaml`. Run `an check spool.yaml` before building. Install the Python dependencies from `headless/requirements.txt` into the interpreter used by An. The check prints the spool name on success and reports invalid fields or YAML syntax on stderr.

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
