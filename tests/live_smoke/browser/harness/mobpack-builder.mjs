import { readFile, readdir } from 'node:fs/promises';
import path from 'node:path';
import { gzipSync } from 'node:zlib';

const TAR_BLOCK_SIZE = 512;

function octal(value, width) {
  const encoded = value.toString(8);
  return encoded.padStart(width - 1, '0') + '\0';
}

function writeString(buffer, offset, length, value) {
  Buffer.from(value).copy(buffer, offset, 0, Math.min(length, Buffer.byteLength(value)));
}

function makeHeader(name, size, mtime) {
  const header = Buffer.alloc(TAR_BLOCK_SIZE, 0);
  writeString(header, 0, 100, name);
  writeString(header, 100, 8, octal(0o644, 8));
  writeString(header, 108, 8, octal(0, 8));
  writeString(header, 116, 8, octal(0, 8));
  writeString(header, 124, 12, octal(size, 12));
  writeString(header, 136, 12, octal(Math.floor(mtime / 1000), 12));
  header.fill(0x20, 148, 156);
  header[156] = '0'.charCodeAt(0);
  writeString(header, 257, 6, 'ustar');
  writeString(header, 263, 2, '00');
  const checksum = header.reduce((sum, byte) => sum + byte, 0);
  writeString(header, 148, 8, octal(checksum, 8));
  return header;
}

function padToTarBlock(buffer) {
  const remainder = buffer.length % TAR_BLOCK_SIZE;
  if (remainder === 0) {
    return buffer;
  }
  return Buffer.concat([buffer, Buffer.alloc(TAR_BLOCK_SIZE - remainder, 0)]);
}

async function collectFiles(rootDir, relativeDir = '') {
  const dirPath = path.join(rootDir, relativeDir);
  const entries = await readdir(dirPath, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const nextRelative = relativeDir ? path.posix.join(relativeDir, entry.name) : entry.name;
    if (entry.isDirectory()) {
      files.push(...(await collectFiles(rootDir, nextRelative)));
      continue;
    }
    if (entry.isFile()) {
      files.push(nextRelative);
    }
  }
  return files.sort();
}

export async function buildFixtureMobpack(fixtureDir) {
  const files = await collectFiles(fixtureDir);
  const parts = [];
  const now = Date.now();

  for (const relativePath of files) {
    const fullPath = path.join(fixtureDir, relativePath);
    const contents = await readFile(fullPath);
    parts.push(makeHeader(relativePath, contents.length, now));
    parts.push(padToTarBlock(contents));
  }

  parts.push(Buffer.alloc(TAR_BLOCK_SIZE, 0));
  parts.push(Buffer.alloc(TAR_BLOCK_SIZE, 0));
  return gzipSync(Buffer.concat(parts));
}
