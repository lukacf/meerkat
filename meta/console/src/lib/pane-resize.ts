export function findPaneResizeRoot(handle: HTMLElement): HTMLElement | null {
  const workbenchRoot = handle.closest("[data-console-workbench]");
  if (workbenchRoot instanceof HTMLElement) return workbenchRoot;
  const shellRoot = handle.closest(".shell");
  return shellRoot instanceof HTMLElement ? shellRoot : null;
}
