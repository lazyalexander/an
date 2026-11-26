"""Trusted controller declaration for the Markdown fixture, including both branches."""


def routes(spool, workspace):
    def output(node, port, op, dest, origin=spool):
        command = {}
        if origin == spool:
            stage = "markdown" if node == "start" else node
            command["producer_argv"] = ["python3", "/spool/code/driver.py", stage, spool]
        return dict(
            operation=op,
            origin=origin,
            source=dict(node=node, port=port),
            destination=dest,
            **command,
        )

    return [
        *(
            dict(
                operation="input",
                origin="task-input:" + name,
                source={"input": name},
                destination=spool,
            )
            for name in ("original.md", "reviews.json")
        ),
        *(
            output(node, "subgraph.json", "publish", "graph-metadata")
            for node in ("start", "route")
        ),
        output("prompt", "request.json", "model", "model:local"),
        output("analysis", "result.json", "handoff", spool, "model:local"),
        *(output("prompt", key + ".txt", "preview", "studio") for key in ("A", "B")),
        output("resolve", "response", "input", spool, "response"),
        *(
            output("revise", name, "publish", workspace)
            for name in ("revised.md", "changes.diff")
        ),
    ]
