const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');

(async () => {
  const input = JSON.parse(fs.readFileSync(0, 'utf8'));
  const script = input.html.match(/<script>([\s\S]*?)<\/script>/)[1];
  const path = '/services/mcp/callback';
  const events = [];
  let location = path + '?' + (input.query || '') + (input.fragment || '');
  const message = {textContent: ''};
  const sandbox = {
    window: {
      location: {search: '?' + (input.query || ''), pathname: path},
      history: {replaceState(state, title, url) {
        events.push({kind: 'replace', url});
        if (input.clear_error) throw new Error('private-error');
        location = url;
      }}
    },
    document: {getElementById(id) { assert.equal(id, 'callback-status'); return message; }},
    async fetch(url, options) {
      assert.equal(location, path, 'Clear URL before any network request');
      if (url === '/__workspace/identity') {
        events.push({kind: 'identity', url, options});
        if (input.identity_error) throw new Error('private-error');
        const status = input.identity_status ?? 200;
        return {ok: status >= 200 && status < 300,
          headers: {get(name) { assert.equal(name, 'content-type'); return input.identity_type ?? 'application/json'; }},
          async json() {
            if (input.identity_json_error) throw new Error('private-error');
            return Object.hasOwn(input, 'identity') ? input.identity : {workspace: 'a'.repeat(64)};
          }};
      }
      assert.equal(url, path);
      events.push({kind: 'post', url, options});
      if (input.post_error) throw new Error('private-error');
      return {status: input.status ?? 202};
    }
  };
  await vm.runInNewContext(script, sandbox, {timeout: 1000});
  process.stdout.write(JSON.stringify({events, message: message.textContent, location}));
})().catch(error => { console.error(error); process.exitCode = 1; });
