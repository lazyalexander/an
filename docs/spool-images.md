# Spool image requirements

An v0.3 uses `/bin/sleep infinity` as the idle process for explicitly loaded instances. The image must supply this executable and support that argument. Registration tests the pinned image in a disposable, network-isolated container with a read-only root filesystem. A failed readiness check prevents registration.

Each guest command ends at a VM stop boundary. An confirms the container has stopped and restarts its idle process before publishing results or accepting another command. The instance ID and mounted data persist across commands; guest processes and in-memory services end at the boundary. This adds a container stop/start to each command.
