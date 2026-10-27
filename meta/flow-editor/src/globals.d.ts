// The shell resolves React and ReactDOM from window globals
// (react-globals.js); the classic JSX transform emits React.createElement
// calls that resolve to them. Mirrors the ambient declaration pattern of
// packages/flow-editor-components/src/globals.d.ts, extended with ReactDOM
// and the data.js boot global.
//
// Migration-window typing (removed by the strictness ratchet): the ambients
// are any-typed and tsconfig sets "types": [] so this program checks
// identifier resolution and module linkage — the load-bearing guard, since
// esbuild silently treats unresolved identifiers as globals — without
// retro-typing the verbatim-moved view components the shell imports. This is
// the same looseness the package programs have, where "react" deliberately
// does not resolve.

declare global {
  const React: any;
  const ReactDOM: any;
  // Boot layout constants assigned to window by data.js, which app.tsx
  // imports for execution order before reading them.
  const META_BOOT: any;

  // Loose JSX for the migration window: the package programs get this for
  // free (no resolvable react means no JSX prop checking); the shell program
  // declares it explicitly so the verbatim-moved components' implicit prop
  // contracts are not retro-typed here.
  namespace JSX {
    type LibraryManagedAttributes<C, P> = any;
    interface IntrinsicElements {
      [name: string]: any;
    }
  }

  interface Window {
    // Back-compat controller surface for browser smokes, live verification
    // scripts, the @flow-editor-components views, and embedders; app.tsx
    // assigns its module-scoped facade here.
    MetaFlowController: any;
    META_BOOT: any;
    // Live verification breadcrumbs the API action handlers publish.
    __metaFlowLastDocument?: any;
    __metaFlowLastDeployPlanTrace?: any;
    __metaFlowLastValidation?: any;
    __metaFlowLastExport?: any;
    __metaFlowLastDeploy?: any;
    __metaFlowLastSource?: any;
    __metaFlowLastImport?: any;
    __metaFlowDisableDownload?: any;
  }
}

export {};
