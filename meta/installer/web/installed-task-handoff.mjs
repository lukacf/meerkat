/** Versioned editable-draft handoff. A fragment never submits or authorizes work. */
export const INSTALLED_TASK_FRAGMENT_PREFIX = '#mobkit-task=';
export const INSTALLED_TASK_FRAGMENT_VERSION = 1;
// Receivers must check this encoded payload budget before decoding or parsing.
export const INSTALLED_TASK_FRAGMENT_MAX = 40000;

export function installedTaskFragment(task) {
  if (typeof task !== 'string' || !task.trim() || task.length > 6000 || Array.from(task).length > 3000 ||
      /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/.test(task) ||
      Array.from(task).some((character) => character.length === 1 && /[\ud800-\udfff]/.test(character))) return null;
  try {
    const payload = encodeURIComponent(JSON.stringify({ version: 1, task }));
    return payload.length <= INSTALLED_TASK_FRAGMENT_MAX ? INSTALLED_TASK_FRAGMENT_PREFIX + payload : null;
  } catch { return null; }
}

// Match the checked interface validators' four-pass decoding bound. Native
// Console aliases are excluded at every layer while authored custom paths keep
// their original encoding in the link.
function taskApplicationPath(value) {
  if (!value.startsWith('/')) return false;
  let decoded = value;
  for (let depth = 0; depth < 4; depth += 1) {
    if (decoded === '/console' || decoded.startsWith('/console/') || decoded.startsWith('//') ||
        /[\\?#\x00-\x20\x7f]/.test(decoded) || decoded.split('/').some((part) => part === '.' || part === '..')) return false;
    let next;
    try { next = decodeURIComponent(decoded); } catch { return false; }
    if (next === decoded) return true;
    decoded = next;
  }
  return false;
}

/** Call only with a URL already checked against the installed interface/tunnel. */
export function installedTaskHandoffUrl(value, task) {
  const fragment = installedTaskFragment(task);
  if (!fragment || typeof value !== 'string' || value.length > 1300 || /[\x00-\x20\x7f]/.test(value)) return null;
  try {
    const url = new URL(value), port = Number(url.port);
    if (url.href !== value || url.protocol !== 'http:' || url.hostname !== '127.0.0.1' ||
        !Number.isInteger(port) || port < 1024 || port > 65535 || url.username || url.password || url.search || url.hash ||
        !taskApplicationPath(url.pathname)) return null;
    return value + fragment;
  } catch { return null; }
}
