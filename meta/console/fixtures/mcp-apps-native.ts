// Native acceptance App. Every action uses the standard SDK host bridge.
import { App } from '@modelcontextprotocol/ext-apps';

const app = new App({ name: 'Native polling result', version: '1.0.0' }, { availableDisplayModes: ['inline'] });
document.body.style.cssText = 'font:14px system-ui;padding:12px;color:#242424;background:#faf9f6;margin:0';
const heading = document.createElement('h3');
const result = document.createElement('p');
const detail = document.createElement('p');
const refresh = document.createElement('button'); refresh.textContent = 'Refresh result';
const toggle = document.createElement('button'); toggle.textContent = 'Start polling';
const status = document.createElement('output'); status.id = 'native-poll-status';
document.body.append(heading, result, detail, refresh, toggle, status);

let viewId = '';
let sequence = 0;
let timer: ReturnType<typeof setInterval> | undefined;
let polling = false;
let inFlight = false;
let refreshActivations = 0;
const completions: { requestId: string; startedAt: number; completedAt: number; latencyMs: number }[] = [];
const errors: string[] = [];
function publishStatus() {
  status.textContent = `${completions.length} polls completed${errors.length ? `; ${errors.join('; ')}` : ''}`;
  status.dataset.state = JSON.stringify({ viewId, polling, inFlight, completions, errors, refreshActivations });
}
function show(value) {
  if (value.isError) throw new Error(`Native App tool failed: ${JSON.stringify(value.content)}`);
  if (!value.structuredContent?.viewId) throw new Error('Native App result lost its view identity');
  viewId = value.structuredContent.viewId;
  heading.textContent = `Records ${viewId}`;
  result.textContent = `${value.structuredContent.count} matching records`;
  detail.textContent = value._meta?.viewDetail ?? '';
  publishStatus();
}
app.ontoolresult = show;
refresh.onclick = async () => {
  refreshActivations += 1;
  refresh.disabled = true;
  try {
    show(await app.callServerTool({ name: 'refresh', arguments: { viewId, requestId: `${viewId}-refresh` } }, { timeout: 5_000 }));
  } catch (error) {
    errors.push(String(error)); publishStatus();
  } finally { refresh.disabled = false; }
};
async function poll() {
  if (!polling || inFlight || !viewId) return;
  inFlight = true;
  const startedAt = Date.now();
  const requestId = `${viewId}-poll-${++sequence}`;
  publishStatus();
  try {
    const value = await app.callServerTool({ name: 'poll', arguments: { viewId, requestId } }, { timeout: 5_000 });
    if (value.structuredContent?.responseMarker !== `APP_POLL_RESULT_${requestId}`) throw new Error('Poll response did not match the request');
    if (value._meta?.pollDetail !== 'PRIVATE_POLL_DETAIL') throw new Error('Poll response lost private metadata');
    show(value);
    const completedAt = Date.now();
    completions.push({ requestId, startedAt, completedAt, latencyMs: completedAt - startedAt });
  } catch (error) {
    errors.push(String(error));
    polling = false;
    clearInterval(timer);
  } finally { inFlight = false; publishStatus(); }
}
toggle.onclick = () => {
  polling = !polling;
  toggle.textContent = polling ? 'Stop polling' : 'Start polling';
  clearInterval(timer);
  if (polling) { void poll(); timer = setInterval(() => void poll(), 2_000); }
  publishStatus();
};
window.addEventListener('pagehide', () => clearInterval(timer));
publishStatus();
await app.connect();
