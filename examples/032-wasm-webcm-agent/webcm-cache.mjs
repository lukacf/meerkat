// File identities from the tree of the signed upstream revision, not a build attestation.
import { createHash } from 'node:crypto';
import { lstat, mkdir, mkdtemp, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const revision = '8afdc285bc278ef1798d0604f83491d14e83c8cf';
const base = `https://raw.githubusercontent.com/edubart/webcm/${revision}`;
const files = [
  ['webcm.mjs', 94587, '6af4afb654e765e93f10866a1e3e44d0701c0524'],
  ['webcm.wasm', 29781613, 'eccac04f50edfb2010f0429d585a5786c16ebf86'],
];

function check(bytes, [name, size, id]) {
  const blob = createHash('sha1').update(`blob ${size}\0`).update(bytes).digest('hex');
  if (bytes.length !== size || blob !== id) throw Error(`WebCM identity mismatch: ${name}`);
}

// A completely absent cache is distinct from a corrupt or partial cache.
export async function verifyCache(directory, { allowAbsent = false, manifest = files } = {}) {
  let present = 0;
  for (const entry of manifest) {
    const [name, size] = entry;
    const path = join(directory, name);
    const stat = await lstat(path).catch(error => {
      if (error.code === 'ENOENT') return null;
      throw error;
    });
    if (!stat) continue;
    if (!stat.isFile() || stat.size !== size) throw Error(`Invalid WebCM cache: ${name}`);
    check(await readFile(path), entry);
    present++;
  }
  if (allowAbsent && present === 0) return false;
  if (present !== manifest.length) throw Error('Missing WebCM cache file(s); use examples.sh --clean');
  return true;
}

async function getBytes(name, size) {
  const response = await fetch(`${base}/${name}`, { redirect: 'error', signal: AbortSignal.timeout(120_000) });
  if (!response.ok) throw Error(`Cannot download ${name}: HTTP ${response.status}`);
  const chunks = [];
  let count = 0;
  for await (const chunk of response.body) {
    count += chunk.length;
    if (count > size) throw Error(`Oversized WebCM download: ${name}`);
    chunks.push(chunk);
  }
  return Buffer.concat(chunks, count);
}

export async function ensureCache(directory, manifest = files, get = getBytes) {
  if (await verifyCache(directory, { allowAbsent: true, manifest })) return;
  await mkdir(directory, { recursive: true });
  // Vite cannot serve the staging directory. An interrupt between the final
  // renames leaves a partial cache, which is rejected on the next launch.
  const stage = await mkdtemp(join(dirname(resolve(directory)), '.webcm-stage-'));
  try {
    for (const entry of manifest) {
      const [name, size] = entry;
      const bytes = await get(name, size);
      check(bytes, entry);
      await writeFile(join(stage, name), bytes);
    }
    for (const [name] of manifest) await rename(join(stage, name), join(directory, name));
  } finally {
    await rm(stage, { recursive: true, force: true });
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  await ensureCache(resolve(process.argv[2]));
}
