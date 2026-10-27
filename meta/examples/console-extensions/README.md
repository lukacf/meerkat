# Custom sidebar panel example

This developer-owned module adds a Results panel to the stock Console sidebar.
It has no widget renderer, tool metadata or MCP Apps dependency.

Build and preview from the repository root:

```sh
npm --prefix console run build
node examples/console-extensions/preview.cjs
```

Open the printed URL and click Results in the sidebar. The panel uses the stock
tabs, splits and saved layout. Closing it disposes its mount. The preview uses
fixture data and requires no provider credentials.

For a product, serve `extension.js` at `/extensions/example.js` on the Console
origin and add the path to `panel_modules` in `console.toml`. Its default export
is an array of panels. Embedded hosts can pass the array as `customPanels` to
`createConsoleApp` or `ConsoleApp` instead. Mounts can host any frontend framework.
