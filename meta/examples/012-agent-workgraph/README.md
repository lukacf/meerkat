# A member completes a WorkGraph item

The writer creates and claims an item, writes a handoff note, attaches a reference to the file and closes the item. Python submits one assignment. It checks the owner in claim history, the event order, the terminal item and the file itself. It never mutates the graph. A reference records where to find the file; WorkGraph does not store its bytes.

Use Python 3.10+ with the `meerkat-meta` package and `rpc_gateway` executable from the same release. Set `META_GATEWAY_BIN` to the absolute executable path and supply `ANTHROPIC_API_KEY` in the environment. Run `python main.py` as an unprivileged account in an isolated workspace: this member uses the shell tool. The gateway uses stdio. The script uses a fresh private directory, removes it on success, and reports its path on failure so you can inspect and then remove it. Do not publish raw runtime state.
