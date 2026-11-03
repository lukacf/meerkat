import { App } from '@modelcontextprotocol/ext-apps';
const app = new App({ name: 'Example result view', version: '1.0.0' }, { availableDisplayModes: ['inline'] });
const body = document.body;
body.style.cssText = 'font:15px system-ui;padding:20px;color:#242424;background:#faf9f6;margin:0;box-sizing:border-box';
const heading = document.createElement('h3'); heading.textContent = 'Search results';
const result = document.createElement('p');
const detail = document.createElement('p');
const button = document.createElement('button'); button.textContent = 'Refresh result';
button.style.cssText = 'font:inherit;padding:8px 12px;border:1px solid #ccc;border-radius:6px;background:white';
body.append(heading, result, detail, button);
function show(value) {
  result.textContent = `${value.structuredContent.count} matching records`;
  detail.textContent = value._meta?.viewDetail ?? '';
}
app.ontoolresult = show;
button.onclick = async () => { button.disabled = true; try { show(await app.callServerTool({ name: 'refresh', arguments: {} })); } finally { button.disabled = false; } };
await app.connect();
