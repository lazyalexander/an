# Spool image requirements

An v0.3 uses `/bin/sleep infinity` as the idle process for explicitly loaded instances. The image must supply this executable and support that argument. Registration tests the pinned image in a disposable, network-isolated container with a read-only root filesystem. A failed readiness check prevents registration.

Each guest command ends at a VM stop boundary. An confirms the container has stopped and restarts its idle process before publishing results or accepting another command. The instance ID and mounted data persist across commands; guest processes and in-memory services end at the boundary. This adds a container stop/start to each command.

## spool/v1

Ship the domain instructions as a nonblank, regular UTF-8 file at `/spool/SKILL.md` (maximum 256 KiB). Put at least one nonempty regular file under the real directory `/spool/code/`. Programs and dependencies may use any language. Describe commands, inputs and declared outputs in the skill. A writable task may copy the generator into `/work`; the packaged baseline is read-only.

Registration checks the native linux/arm64 variant in an isolated disposable container, then streams the stopped filesystem export on the host. The admission record pins the image digest, platform variant digest, contract version and skill digest. Re-register legacy images before loading or dispatching new commands. Task state lists authorized image admission summaries, and the task-scoped MCP tool `an_skill(spool)` reads the admitted instructions before load. A successful admission describes structure and runtime compatibility; domain acceptance belongs to the spool author and agent.

Probe and task containers explicitly select the admitted platform. Unload and reload instances created before platform pinning. Export paths may use up to 4096 bytes each. The scanner accepts up to 16,384 entries under /spool, including the directory itself, with a combined path budget of 4 MiB.
