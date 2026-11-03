// Native Console and protected setup have separate DOM and lifetime owners.
const mounted = new WeakMap();
const FOCUSABLE = 'button:not(:disabled), input:not(:disabled), textarea:not(:disabled), select:not(:disabled), a[href], [tabindex]:not([tabindex="-1"])';

export function mountConsoleShell({ mountConsoleSetup, mountSetupConversation, loadSetup,
  mountControllerReview, loadControllerReview, document: doc = globalThis.document,
  matchMedia, setupOptions = {}, controllerReviewOptions = {} } = {}) {
  if (!doc || (loadSetup !== undefined && typeof loadSetup !== 'function') ||
      (loadControllerReview !== undefined && typeof loadControllerReview !== 'function') ||
      (mountControllerReview !== undefined && typeof mountControllerReview !== 'function') ||
      ((mountConsoleSetup !== undefined || mountSetupConversation !== undefined) &&
       (typeof mountConsoleSetup !== 'function' || typeof mountSetupConversation !== 'function'))) throw new Error('Qualified Console presentation is unavailable.');
  if (mounted.has(doc)) return mounted.get(doc);
  const native = doc.getElementById('root');
  if (!native || native.parentNode !== doc.body) throw new Error('The native Console root is unavailable.');
  const win = doc.defaultView;
  const hasSetup = typeof loadSetup === 'function' || typeof mountConsoleSetup === 'function';
  const hasReview = typeof loadControllerReview === 'function' || typeof mountControllerReview === 'function';
  const hasPanel = hasSetup || hasReview;
  // A sibling 480px drawer leaves at least 960px for the real native workbench.
  const media = hasPanel ? (matchMedia ?? win.matchMedia.bind(win))('(max-width: 1439px)') : null;
  const originalNext = native.nextSibling, originalPageClass = doc.body.classList.contains('forge-console-shell-page');
  const element = (tag, className, text) => { const node = doc.createElement(tag); node.className = className; if (text) node.textContent = text; return node; };
  const shell = element('div', 'forge-console-shell');
  shell.dataset.setup = String(hasSetup);
  if (hasReview) shell.dataset.review = 'true';
  const toolbar = element('div', 'forge-console-shell__toolbar'); toolbar.setAttribute('role', 'region'); toolbar.setAttribute('aria-label', 'Console controls');
  const brand = element('span', 'forge-console-shell__brand', 'Your agents');
  const mark = element('span', 'forge-console-shell__mark'); mark.setAttribute('aria-hidden', 'true'); brand.prepend(mark);
  toolbar.append(brand);
  let setupTrigger = null, reviewTrigger = null;
  if (hasSetup) {
    const hint = element('span', 'forge-console-shell__hint', 'App setup');
    setupTrigger = element('button', 'forge-console-shell__open', 'Set up your app'); setupTrigger.type = 'button';
    setupTrigger.setAttribute('aria-controls', 'forge-console-setup-drawer'); setupTrigger.setAttribute('aria-expanded', 'false');
    toolbar.append(hint, setupTrigger);
  }
  if (hasReview) {
    reviewTrigger = element('button', 'forge-console-shell__open', 'Review setup'); reviewTrigger.type = 'button';
    reviewTrigger.setAttribute('aria-controls', 'forge-console-setup-drawer'); reviewTrigger.setAttribute('aria-expanded', 'false');
    toolbar.append(reviewTrigger);
  }
  const logout = element('form', 'forge-console-shell__logout'); logout.method = 'POST'; logout.action = '/logout';
  const signOut = element('button', 'forge-console-shell__sign-out', 'Sign out'); signOut.type = 'submit'; logout.append(signOut);
  toolbar.append(logout);
  const workspace = element('div', 'forge-console-shell__workspace');
  const nativeArea = element('div', 'forge-console-shell__native');
  let shade = null, drawer = null;
  if (hasPanel) {
    shade = element('div', 'forge-console-shell__shade'); shade.hidden = true; shade.setAttribute('aria-hidden', 'true');
    drawer = element('aside', 'forge-console-shell__drawer'); drawer.id = 'forge-console-setup-drawer'; drawer.hidden = true; drawer.tabIndex = -1;
    drawer.setAttribute('aria-label', 'Protected app setup');
  }
  doc.body.classList.add('forge-console-shell-page'); doc.body.insertBefore(shell, native); nativeArea.append(native);
  workspace.append(nativeArea); if (hasPanel) workspace.append(shade, drawer); shell.append(toolbar, workspace);
  let view = null, opened = false, disposed = false, modal = false, priorFocus = null, opening = 0;
  let selectedView = null, authenticationEnded = false;
  const triggerFor = kind => kind === 'controller-review' ? reviewTrigger : setupTrigger;
  const held = new Map();
  function hideBackground() {
    for (const node of [...doc.body.children, ...shell.children]) {
      if (node === shell || node === workspace || ['SCRIPT', 'LINK', 'STYLE'].includes(node.tagName)) continue;
      if (!held.has(node)) held.set(node, { inert: node.getAttribute('inert'), aria: node.getAttribute('aria-hidden') });
      node.setAttribute('inert', ''); node.setAttribute('aria-hidden', 'true');
    }
    if (!held.has(nativeArea)) held.set(nativeArea, { inert: nativeArea.getAttribute('inert'), aria: nativeArea.getAttribute('aria-hidden') });
    nativeArea.setAttribute('inert', ''); nativeArea.setAttribute('aria-hidden', 'true');
  }
  function restoreBackground() {
    for (const [node, attributes] of held) {
      for (const [name, value] of [['inert', attributes.inert], ['aria-hidden', attributes.aria]]) {
        if (value === null) node.removeAttribute(name); else node.setAttribute(name, value);
      }
    }
    held.clear();
  }
  function focusDrawer() { (drawer.querySelector('[tabindex="-1"]') ?? drawer).focus(); }
  function geometry() {
    if (!hasPanel) { shell.dataset.modal = 'false'; return; }
    const next = opened && media.matches;
    shell.dataset.modal = String(next); drawer.setAttribute('role', next ? 'dialog' : 'complementary');
    if (next) {
      drawer.setAttribute('aria-modal', 'true'); hideBackground();
      if (!modal && !drawer.contains(doc.activeElement)) focusDrawer();
    } else { drawer.removeAttribute('aria-modal'); restoreBackground(); }
    modal = next; shade.hidden = !next;
  }
  function close() {
    if (!opened || disposed) return;
    const trigger = triggerFor(selectedView);
    opened = false; opening++; view?.dispose(); view = null; drawer.replaceChildren(); drawer.hidden = true;
    shell.dataset.open = 'false'; trigger.setAttribute('aria-expanded', 'false'); geometry();
    if (priorFocus?.isConnected && !priorFocus.closest('[inert]')) priorFocus.focus(); else trigger.focus();
    priorFocus = null;
    selectedView = null; delete drawer.dataset.view;
  }
  function notice(text) {
    const message = element('p', 'forge-console-shell__error', text);
    const button = element('button', 'forge-console-shell__open', selectedView === 'controller-review' ? 'Close review' : 'Close setup'); button.type = 'button'; button.addEventListener('click', close);
    drawer.replaceChildren(message, button);
  }
  async function openPanel(kind) {
    if (disposed || authenticationEnded || (kind === 'controller-review' ? !hasReview : !hasSetup)) return;
    const incomingFocus = doc.activeElement;
    if (opened) { if (selectedView === kind) return; close(); }
    const review = kind === 'controller-review', trigger = triggerFor(kind);
    const selectedOpening = ++opening;
    selectedView = kind; drawer.dataset.view = kind;
    drawer.setAttribute('aria-label', review ? 'Protected setup review' : 'Protected app setup');
    priorFocus = incomingFocus; opened = true; drawer.hidden = false; shell.dataset.open = 'true'; trigger.setAttribute('aria-expanded', 'true');
    try {
      let selected = { mountConsoleSetup, mountSetupConversation, mountControllerReview };
      const load = review ? loadControllerReview : loadSetup;
      if (load) {
        notice(review ? 'Loading protected review...' : 'Loading app setup...'); geometry(); focusDrawer();
        selected = await load();
        // A close, disposal or successor opening cannot initialize late setup
        // work from a previously selected module load.
        if (!opened || disposed || authenticationEnded || opening !== selectedOpening) return;
      }
      if (review ? typeof selected?.mountControllerReview !== 'function'
          : typeof selected?.mountConsoleSetup !== 'function' || typeof selected?.mountSetupConversation !== 'function') throw new Error('Qualified presentation modules are unavailable.');
      drawer.replaceChildren();
      if (review) {
        const controls = element('div', 'forge-console-shell__review-toolbar');
        const heading = element('span', '', 'Setup approval');
        const button = element('button', 'forge-console-shell__open', 'Close'); button.type = 'button';
        button.setAttribute('aria-label', 'Close review drawer'); button.addEventListener('click', close);
        controls.append(heading, button);
        const content = element('div', 'forge-console-shell__review-content');
        drawer.append(controls, content);
        view = selected.mountControllerReview(content, controllerReviewOptions);
      } else {
        view = selected.mountConsoleSetup(drawer, { ...setupOptions, mountSetupConversation: selected.mountSetupConversation, focus: false, onClose: close });
      }
    }
    catch {
      if (!opened || disposed || authenticationEnded || opening !== selectedOpening) return;
      // No host request is replayed because presentation could not mount.
      notice(review ? 'The protected review could not be displayed. Close this panel to keep using Console.' : 'Setup could not be displayed. Close this panel and reopen it to observe current status.');
    }
    if (opened && !disposed && !authenticationEnded && opening === selectedOpening) { geometry(); focusDrawer(); }
  }
  const open = () => openPanel('setup');
  const openControllerReview = () => openPanel('controller-review');
  function clearAuthentication() {
    if (!hasReview || authenticationEnded) return;
    // The form event precedes its native POST. Fence a pending module load
    // synchronously even when the review handle has not been constructed.
    authenticationEnded = true; opening++;
    if (selectedView === 'controller-review') view?.clearAuthentication();
  }
  function keydown(event) {
    if (!opened || disposed) return;
    if (event.key === 'Escape' && (modal || drawer.contains(event.target))) { event.preventDefault(); close(); return; }
    if (!modal || event.key !== 'Tab') return;
    const controls = [...drawer.querySelectorAll(FOCUSABLE)].filter(node => !node.closest('[hidden]') && node.getAttribute('aria-hidden') !== 'true');
    if (!controls.length) { event.preventDefault(); drawer.focus(); return; }
    const first = controls[0], last = controls.at(-1), active = doc.activeElement;
    if (event.shiftKey && (active === first || !controls.includes(active))) { event.preventDefault(); last.focus(); }
    else if (!event.shiftKey && (active === last || !controls.includes(active))) { event.preventDefault(); first.focus(); }
  }
  function focusin(event) { if (modal && opened && !drawer.contains(event.target)) focusDrawer(); }
  setupTrigger?.addEventListener('click', open); reviewTrigger?.addEventListener('click', openControllerReview);
  logout.addEventListener('submit', clearAuthentication); shade?.addEventListener('click', close);
  doc.addEventListener('keydown', keydown, true); doc.addEventListener('focusin', focusin);
  media?.addEventListener('change', geometry);
  shell.dataset.open = 'false'; geometry();
  function dispose() {
    if (disposed) return;
    close(); disposed = true; restoreBackground();
    logout.removeEventListener('submit', clearAuthentication);
    doc.removeEventListener('keydown', keydown, true); doc.removeEventListener('focusin', focusin); media?.removeEventListener('change', geometry);
    win.removeEventListener('pagehide', dispose);
    doc.body.insertBefore(native, originalNext?.parentNode === doc.body ? originalNext : shell); shell.remove();
    if (!originalPageClass) doc.body.classList.remove('forge-console-shell-page'); mounted.delete(doc);
  }
  win.addEventListener('pagehide', dispose, { once: true });
  const handle = { open, openControllerReview, close, clearAuthentication, dispose }; mounted.set(doc, handle); return handle;
}
