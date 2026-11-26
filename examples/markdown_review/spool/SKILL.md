Review original.md against two proposed paragraph edits in reviews.json. Ask the author to resolve incompatible edits. Deliver the revised Markdown and unified diff.

For a full review, submit one expand node named `start` with argv
`["python3","/spool/code/driver.py","markdown",SPOOL]`, substituting the admitted
image reference as SPOOL. It takes no initial bindings and produces subgraph.json.
The driver uses a local model and opens the declared resolve panel on conflict.

For other text tasks the image also provides a general compositor. Use a run
with argv `["python3","/spool/code/compose.py",OUTPUT_NAME,SECTIONS_JSON]`.
Choose a .md output name and declare it as the node output. The last argument
is a JSON array containing your Markdown strings and/or {"file":"PORT/FILENAME"}
objects in the desired order. Bind existing artifacts to any declared input
ports (source, reviews, analysis, choice). Input references in the text are
relative to /input. Each choice preview from the prompt node contains the
original paragraph, replacement and reason. A delivery may bind an original
task input directly to preserve its bytes.
