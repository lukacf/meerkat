import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { chromium } from 'playwright';

import { startBrowserSmokeServer } from './server.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

function parseArgs(argv) {
  const scenarioIds = [];
  let headed = false;

  for (let index = 0; index < argv.length; index += 1) {
    const token = argv[index];
    if (token === '--headed') {
      headed = true;
      continue;
    }
    if (token === '--scenario') {
      const value = argv[index + 1];
      if (!value) {
        throw new Error('--scenario requires a value');
      }
      scenarioIds.push(value);
      index += 1;
      continue;
    }
    throw new Error(`unknown argument: ${token}`);
  }

  return { headed, scenarioIds };
}

async function waitForSmokeApi(page) {
  await page.waitForFunction(() => Boolean(window.__liveSmoke), undefined, {
    timeout: 10_000,
  });
}

async function main() {
  const { headed, scenarioIds } = parseArgs(process.argv.slice(2));
  const server = await startBrowserSmokeServer();
  const browser = await chromium.launch({ headless: !headed });
  const page = await browser.newPage();

  page.on('console', (message) => {
    console.log(`[browser:${message.type()}] ${message.text()}`);
  });

  try {
    const url = `${server.origin}/index.html`;
    console.log(`browser live smoke server: ${server.origin}`);
    console.log(`loading ${url}`);
    await page.goto(url, { waitUntil: 'networkidle' });
    await waitForSmokeApi(page);
    const summary = await page.evaluate(async (ids) => window.__liveSmoke.run(ids), scenarioIds);
    for (const result of summary.results) {
      const detail = result.error ? `\n${result.error}` : '';
      console.log(`${result.status.toUpperCase()} ${result.id} (${result.duration_ms}ms)${detail}`);
    }
    if (!summary.ok) {
      process.exitCode = 1;
    }
  } finally {
    await page.close();
    await browser.close();
    await server.close();
  }
}

main().catch((error) => {
  console.error(error instanceof Error ? error.stack ?? error.message : String(error));
  process.exitCode = 1;
});
