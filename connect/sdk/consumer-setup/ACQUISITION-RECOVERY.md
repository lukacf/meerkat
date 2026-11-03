# Continue after an acquisition host exits

Python, Node and Compose acquisition jobs retain a request before starting their
effects. Save the returned `recovery_request.id` and `recovery_request.sha256`
together with the original workspace path. The digest binds the request, original
staging root and frozen host inputs; it is not a publisher signature.

If the setup host exits, first establish that its worker and children have stopped
and that the retained workspace and staging root are under exclusive host custody.
Then start the current setup host with a new workspace:

```sh
"$MCP_PYTHON" -I -B "$KIT/setup/mcp_adapter.py" \
  --workspace /ABS/new-readback-workspace \
  --families '' --python "$MCP_PYTHON" \
  --recover-workspace /ABS/original-workspace \
  --recover-request ORIGINAL_RECOVERY_ID \
  --recover-request-sha256 ORIGINAL_RECOVERY_SHA256
```

The host interpreter needs the supported MCP SDK. No acquisition profile,
installer, pip, npm or engine executable is needed for this readback. The current
setup code reads the original frozen host inputs as data and does not execute
them. The original workspace and staging root must remain at their original
canonical paths. An optional current profile may confirm the original pin; it
cannot replace the original request or grant new authority.

By default, only `setup_state` and `setup_reconcile` are exposed. `setup_state` reports the
original job as `unconfirmed`, with `recovered: true`. Call `setup_reconcile` with
that job ID to obtain a separate observation:

| Observation | Meaning |
| --- | --- |
| `matched` | The required final receipt, request associations and retained file hashes match. The result contains the original acquisition result shape. |
| `unconfirmed` | The check could not establish a match. Preserve the files; this does not prove that no effects occurred. |

The scope is `retained_acquisition_files_only`. Checking does not download,
pull images, install, repair, retry or rewrite the original job status. The browser
companion provides the same action as **Check acquired files**. Opening the page
does not automatically start a check.

For Python and Node, a matching observation returns the staging ID and retained
package inputs. If source research was interrupted, the current host can also
enable [read-only archive inspection](ACQUIRED-INSPECTION.md#continue-research-after-restart)
with `--allow-acquired-inspection` and an explicitly supplied matching selected
profile. This optional path requires its current pinned tools and the current
host Python, but no original pip/npm/Node runtime in acquisition-only recovery.
Mixed selections retain their other request owners' runtime requirements. It adds only
`setup_inspect_acquired`; the exact job must first have a matching reconciliation
and all retained inputs are rechecked around every read. Without that opt-in,
the data-only command above remains unchanged.

Open a separately authorized normal setup session using that
staging root, author or inspect `package.json` and `activation.json`, and continue
through `setup_selected_check` and `setup_selected_install`. These metadata files
are allowed alongside acquired inputs but are not validated by acquisition
readback. The canonical selected-package validator remains their owner.

Node acquisitions with an explicit Linux target also retain the target and native
runtime observations. Readback checks these against the frozen profile without
probing the current machine. The selected Node installer remains responsible for
installation on the current host; matching acquired files is not a compatibility
or readiness check. See [Node acquisition](NODE-ACQUISITION.md).

For Compose, a matching observation requires the final `proposal.json` and all
of its bound files. An image acquisition record alone is insufficient. Readback
does not contact the engine or establish that images are still available. The
host separately admits the proposal through its existing execution path, which
performs its own engine and image checks.

Missing final receipts, partial publication, changed files and crossed roots or
request bindings stay unconfirmed. Older acquisition workspaces without this
retained request cannot be upgraded by inferring success from their directories.
Keep them for inspection; a new attempt requires an explicit new request and
destination.

MCPB acquisitions use this same original-request recovery mechanism. A matching
observation proves retained bundle bytes only. To resume manifest inspection,
select the same current selected profile and opt in to acquired inspection;
`setup_inspect_mcpb` then accepts only an exact reconciled job/receipt pair.
Arbitrary staging reads and download/install actions stay unavailable. See
[MCPB acquisition](MCPB-ACQUISITION.md) for the complete flow.
