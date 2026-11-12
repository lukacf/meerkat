# GitHub selected reads

This official-github/selected-reads profile carries the GitHub MCP server as its
publisher released it for macOS arm64: the executable is a member of the upstream
release archive, unchanged. The delivery record beside this file names the
release version, the source revision of its tag, the size and SHA-256 of that
release archive, and the SHA-256 of the executable.

The route is fixed. It serves GitHub.com in read-only mode with exactly these
three tools: get_me, list_commits, get_file_contents. Select all three. Custom
hosts, other tools, extra flags and environment values are not accepted by this
installation profile.

Configuration has exactly profile, operations and config. The config has exactly
token_env, and its value is GITHUB_PERSONAL_ACCESS_TOKEN. Enter the variable name
only; never put a token in the configuration. The host supplies its protected
token to that variable of the child process and controls the whole child
environment. An empty env object does not remove inherited variables.

get_me is the server's authenticated-user read. A successful provider response
is still required to observe identity. Tool selection is a ceiling on tools, not
a restriction on repository arguments: the caller's own programs and the
provider's permissions decide which repositories can be read.

Template placeholders are metadata only and must never be launched. The launch
arguments and the working directory are written by the installer for the chosen
destination and are checked again on readback.

Installation and readback require macOS arm64. Assembly, configuration checking,
installation and readback do not execute the server, authenticate, read a
secret, call the provider or establish readiness. Live schema, account and
repository qualification remain separate.

The upstream release archive includes its MIT LICENSE, which is carried under
notices. That archive supplies no further dependency notices, and this route
does not claim a complete dependency-notice audit.
