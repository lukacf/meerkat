# Playwright isolated Chrome

This official-playwright/isolated-chrome profile carries the Playwright MCP
server as its publisher released it on the public npm registry: three package
archives, unchanged, under archives. The project file, the lock and the closure
record beside them are written from those archives and bind each one by its
registry integrity value. The delivery record names the versions and the source
revision of the server package.

No browser is distributed. The host selects an existing Chrome executable and
pins it by SHA-256 in a host profile, together with a short, host-controlled
directory for the server's working files. The contract for that profile is in
the library's host preparation guide.

The whole Playwright endpoint is exposed. The five listed operations
(browser_navigate, browser_snapshot, browser_fill_form, browser_click,
browser_close) describe the coverage this profile was adopted for; they are not
a tool filter. The profile accepts no custom fields: its configuration has
exactly profile, operations and an empty config.

The server starts headless, with an isolated browser profile, with the selected
Chrome executable and with fixed action and navigation timeouts. It imports no
logged-in state, cookies, credentials, remote browser or extension connection.
The isolation flags are not a security boundary: the host owns browser lifetime,
file and network policy, access to snapshot files, and cleanup after all child
processes have ended.

Launch only with a host-controlled minimal environment. Unrelated Playwright,
Node and proxy settings in the parent environment can change behaviour, and the
env object of an MCP configuration does not clear inherited values.

Installation requires macOS arm64 and the Node and npm versions the installer
names in its own help. It installs the three archives offline from this bundle;
no package is downloaded. Template placeholders are metadata only and must never
be launched.

Assembly, configuration checking and readback do not start Node, npm, the server
or a browser, and do not establish browser compatibility or readiness. The
licence and notice files of the three packages are carried under notices.
