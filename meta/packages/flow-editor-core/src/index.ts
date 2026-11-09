// @flow-editor-core — the Meta Flow Editor's headless controller plane.
//
// Pure projection/transition logic extracted from the legacy
// flow-editor/src/controller.js monolith, module by module. Framework-free:
// no DOM access at load time, no React. The shell bundles this package as
// the window.MetaFlowCore IIFE and the controller.js bootstrap builds
// window.MetaFlowController from createMetaFlowController.
export * from "./catalogs/hydration";
export * from "./contract/options";
export * from "./controller-facade";
export * from "./document/build-projection";
export * from "./domain/tool-skill-access";
export * from "./drafts/mob-settings";
export * from "./editors/agent-editor";
export * from "./editors/basic-editor";
export * from "./editors/graph-editor";
export * from "./flow/launch-modes";
export * from "./flow/reconcile";
export * from "./flow/step-tree";
export * from "./members/patches";
export * from "./registry/flow-registry";
export * from "./rpc/client";
export * from "./rpc/doc-ops";
export * from "./schema/field-edit";
export * from "./shared/constants";
export * from "./shared/normalize";
export * from "./shell/outcomes";
export * from "./source/view";
export * from "./studio/state";
export * from "./views/view-config";
