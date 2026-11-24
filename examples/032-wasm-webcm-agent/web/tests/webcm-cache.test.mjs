import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, readFile, readdir, rm, symlink, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { ensureCache, verifyCache } from '../../webcm-cache.mjs';

const data = new Map([['webcm.mjs', Buffer.from('synthetic js')], ['webcm.wasm', Buffer.from('synthetic guest')]]);
const manifest = Array.from(data, ([name, bytes]) => [name, bytes.length,
  createHash('sha1').update(Buffer.concat([Buffer.from(`blob ${bytes.length}\0`), bytes])).digest('hex')]);

for (const shape of ['good', 'js-only', 'wasm-only', 'wrong-size', 'wrong-id', 'symlink', 'bad-download', 'short-download', 'interrupted']) {
  test(`WebCM pinned pair: ${shape}`, async t => {
    const root = await mkdtemp(join(tmpdir(), 'webcm-test-'));
    t.after(() => rm(root, { recursive: true, force: true }));
    const dir = join(root, 'public'); await mkdir(dir);
    const js = join(dir, 'webcm.mjs'), wasm = join(dir, 'webcm.wasm');
    let calls = 0;
    const get = async name => {
      calls++;
      if (name === 'webcm.wasm') {
        if (shape === 'bad-download') return Buffer.alloc(data.get(name).length);
        if (shape === 'short-download') return Buffer.from('x');
        if (shape === 'interrupted') throw Error('synthetic transfer failure');
      }
      return data.get(name);
    };
    assert.equal(await verifyCache(dir, { manifest, allowAbsent: true }), false);
    await assert.rejects(verifyCache(dir, { manifest }));
    if (shape === 'good') {
      await ensureCache(dir, manifest, get); await ensureCache(dir, manifest, get);
      assert.equal(calls, 2);
      assert.deepEqual(await readFile(js), data.get('webcm.mjs'));
      assert.deepEqual(await readFile(wasm), data.get('webcm.wasm'));
      assert.equal(await verifyCache(dir, { manifest }), true);
    } else if (['bad-download', 'short-download', 'interrupted'].includes(shape)) {
      await assert.rejects(ensureCache(dir, manifest, get));
      assert.equal(calls, 2); assert.deepEqual(await readdir(dir), []);
      assert.deepEqual(await readdir(root), ['public']);
    } else {
      if (shape !== 'wasm-only') await writeFile(js, data.get('webcm.mjs'));
      if (shape === 'symlink') await symlink(js, wasm);
      else if (shape !== 'js-only') await writeFile(wasm, shape === 'wrong-size' ? '' :
        shape === 'wrong-id' ? Buffer.alloc(data.get('webcm.wasm').length) : data.get('webcm.wasm'));
      await assert.rejects(ensureCache(dir, manifest, get)); assert.equal(calls, 0);
    }
  });
}
