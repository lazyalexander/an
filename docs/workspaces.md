# Workspace bindings

Register the spool images and local MCP services first. Create a workspace with digest-pinned spool references and optional edit and tool grants.

```sh
an workspace create --spool "$IMAGE" --editable "$IMAGE" --allow-tool editor/read
an workspace bind "$WORKSPACE" --harness local --session "$SESSION_ID"
```

Use the returned stdio MCP configuration for that session. Trusted hosts without native session IDs can supply a stable `--key` instead. Bindings are scoped by harness and identity type. The first connection creates a private task. Reconnecting resumes it. Use `an task new --harness local --session "$SESSION_ID"` to start fresh after unloading its instances.

Within MCP, `an_state` identifies the current workspace and task. `an_deliveries` lists committed outputs from tasks in this workspace, fifty per page. Pass the returned next cursor as after. Supply the selected delivery ID as the input to `an_run`. The receiving task sees the declared outputs; its editable generator, instances and logs remain private.
