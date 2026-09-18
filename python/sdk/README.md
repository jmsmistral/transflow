# SDK module

`transflow` contains the typed SDK API and the shared distribution version.
It is packaged together with `transflow_worker` by [the Python manifest](../pyproject.toml).
Importing it does not discover workspaces, read datasets, load engines or start
services. Currently only bootstrap version/protocol metadata is available;
transform declarations and catalogue APIs remain later tasks.
