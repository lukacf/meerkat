# Organization starting examples

This installable, fictional Python plugin changes Forge's opening examples and
the context available to its guide. It demonstrates the version 1 organization
contribution without depending on private services.

Install it into the **same Python environment that runs Forge**. For an existing
Forge installation using the normal launcher, run these commands from the folder
containing `start.sh`:

```sh
.venv/bin/python -m pip install /path/to/meerkat-meta/installer/examples/workplace-starters
META_FORGE_ORGANIZATION=workplace bash start.sh
```

For a source development environment where Forge is already installed:

```sh
python -m pip install ./installer/examples/workplace-starters
META_FORGE_ORGANIZATION=workplace python -m uvicorn launchkit.server:app --host 127.0.0.1 --port 8787
```

The command assumes Forge itself is installed in that environment. Use the same
environment variable with your existing Forge launcher. Distribution installers
may need to install the plugin into their packaged Python environment.

The host selects one entry-point name from `meerkat_forge.organizations.v1`.
Forge calls that installed function once at startup and validates its
returned data. Missing, ambiguous or invalid explicit selections, including
an empty value, stop startup. They do not fall back to unrelated stock
examples. No plugin is loaded by default.

The fields are documented by `launchkit.organization.OrganizationPlugin`:

- `schema_version`: 1.
- `id`, `version`, `label`: the contribution's identity and visible label.
- `description`: optional opening copy.
- `guide_context`: optional context for the guide.
- `starting_examples`: up to eight examples, each with an `id`, `title`,
  `outcome` and `initial_brief`. Optional `context` and `integration_hints`
  explain relevance to the guide.

An empty example list intentionally leaves only the user's own idea. Selecting
an example puts its brief in the ordinary composer; sending remains an explicit
user action. Existing writing is preserved and examples can be previewed or
deliberately appended.

The complete contribution is copied into each new saved setup. Resuming that
setup preserves its original examples and guide context even if the host's
plugin version changes. New setups use the currently selected version.

All contribution fields may be shown to that setup's viewers and sent to its
model. Keep credentials, tokens and confidential operational configuration out
of this object. Only trusted operators install and select Python packages.
The selection is host-wide, not a tenant identity or authorization boundary.

This contribution offers context and ideas, not deployment, account setup,
installed integrations, permissions or a fixed agent design. Those remain
separate operations owned by Forge, Connect and the native runtime. Integration
hints do not claim that a connector is installed or available. Hosted OAuth
sign-in and service writes are not supported yet; this example connects no live
application.
