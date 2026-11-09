const RESIZE_LOCK_DATA_KEY = "ccResizeLockCount";
const RESIZE_STATE_DATA_KEY = "ccResizing";

function resizeLockRoot(): HTMLElement | null {
  if (typeof document === "undefined") {
    return null;
  }

  return document.documentElement;
}

function readResizeLockCount(): number {
  const root = resizeLockRoot();
  if (!root) {
    return 0;
  }

  const raw = root.dataset[RESIZE_LOCK_DATA_KEY];
  const count = Number(raw);
  return Number.isFinite(count) && count > 0 ? count : 0;
}

export function acquireResizeLock(): void {
  const root = resizeLockRoot();
  if (!root) {
    return;
  }

  const nextCount = readResizeLockCount() + 1;
  root.dataset[RESIZE_LOCK_DATA_KEY] = String(nextCount);
  root.dataset[RESIZE_STATE_DATA_KEY] = "true";
}

export function releaseResizeLock(): void {
  const root = resizeLockRoot();
  if (!root) {
    return;
  }

  const nextCount = Math.max(0, readResizeLockCount() - 1);
  if (nextCount === 0) {
    delete root.dataset[RESIZE_LOCK_DATA_KEY];
    delete root.dataset[RESIZE_STATE_DATA_KEY];
    return;
  }

  root.dataset[RESIZE_LOCK_DATA_KEY] = String(nextCount);
}
