import * as esbuild from 'esbuild';
import fs from 'node:fs/promises';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { builtinModules } from 'node:module';
import { sourceProjections } from './source-projections.mjs';
import { effectSdkProjection, effectFactoryPrelude } from './effect-sdk-projection.mjs';
import { sdkPortManifest } from './sdk-port-manifest.mjs';

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const manifest = JSON.parse(await fs.readFile(path.join(root, 'vendor/SOURCE-MANIFEST.json'), 'utf8'));
for (const item of manifest.files) {
  const bytes = await fs.readFile(path.join(root, 'vendor/activepieces', item.path));
  if (digest(bytes) !== item.sha256) throw new Error(`Upstream source digest mismatch: ${item.path}`);
}
const packageInfo = JSON.parse(await fs.readFile(path.join(root, 'package.json'), 'utf8'));
for (const [name, version] of Object.entries({ ...packageInfo.devDependencies, ...packageInfo.dependencies })) {
  const installed = JSON.parse(await fs.readFile(path.join(root, 'node_modules', name, 'package.json'), 'utf8'));
  if (installed.version !== version) throw new Error(`Dependency version mismatch: ${name}`);
}
await fs.mkdir(path.join(root, 'dist'), { recursive: true });
execFileSync(process.execPath, [path.join(root, 'node_modules/typescript/bin/tsc'), '-p', 'tsconfig.json'], { cwd: root, stdio: 'inherit' });
await fs.chmod(path.join(root, 'dist/mcp-cli.js'), 0o755);
const source = path.join(root, 'vendor/activepieces');
const output = await esbuild.build({
  absWorkingDir: root, entryPoints: ['bundle/entry.ts'], outfile: 'dist/donor-factory.js', bundle: true,
  platform: 'node', format: 'cjs', target: 'node22', treeShaking: true, minify: true, metafile: true,
  legalComments: 'external',
  plugins: [await sourceProjections(root), await effectSdkProjection(root)],
  alias: {
    '@slack/web-api': path.join(root, 'bundle/slack-seam.ts'),
    '@linear/sdk': path.join(root, 'node_modules/@linear/sdk/dist/index-es.js'),
    'monday-sdk-js': path.join(root,'node_modules/monday-sdk-js/server-sdk.js'),
    '@mailchimp/mailchimp_marketing': path.join(root,'bundle/mailchimp-sdk-port.ts'),
    '@activepieces/pieces-framework': path.join(source, 'packages/pieces/framework/src/index.ts'),
    '@activepieces/core-utils': path.join(source, 'packages/core/utils/src/index.ts'),
    '@activepieces/core-piece-types': path.join(source, 'packages/core/piece-types/src/index.ts'),
    '@activepieces/pieces-common': path.join(root, 'bundle/common-seam.ts'),
    '@notionhq/client': path.join(root, 'bundle/notion-seam.ts'),
    'intercom-client': path.join(root, 'bundle/intercom-seam.ts'),
    '@googleapis/sheets': path.join(root, 'bundle/google-sheets-seam.ts'),
    '@googleapis/docs': path.join(root, 'bundle/google-docs-seam.ts'),
    '@googleapis/drive': path.join(root, 'bundle/google-drive-seam.ts'),
    'contentful-management': path.join(root, 'bundle/contentful-seam.ts'),
    '@microsoft/microsoft-graph-client': path.join(root, 'bundle/graph-seam.ts'),
    '@hubspot/api-client': path.join(root, 'bundle/hubspot-seam.ts'),
    '@supabase/supabase-js': path.join(root, 'bundle/supabase-admin-port.ts'),
  },
  banner: { js: 'import { createRequire } from "node:module";\nconst actualRequire = createRequire(import.meta.url);\nexport function createDonorBundle(__toolkitTransport, __toolkitContext = {invalidResponse(){throw new Error("Invalid SDK response");}}) {\nconst require = name => { const actual = actualRequire(name); if (["http","https","node:http","node:https"].includes(name)) { const denied = () => { throw new Error("Unowned HTTP refused"); }; return { ...actual, request: denied, get: denied }; } return actual; };\nconst module = { exports: {} };\nconst exports = module.exports;' + effectFactoryPrelude },
  footer: { js: 'return module.exports;\n}' },
});
await sdkPortManifest(root);
const result = output.metafile.outputs['dist/donor-factory.js'];
const unexpectedImports = result.imports.filter(item => !builtinModules.includes(item.path) && !builtinModules.includes(item.path.replace(/^node:/, '')));
if (unexpectedImports.length) throw new Error(`Unexpected external donor import: ${unexpectedImports.map(item => item.path).join(', ')}`);
const bundle = await fs.readFile(path.join(root, 'dist/donor-factory.js'));
if (bundle.includes(Buffer.from('NODE_TLS_REJECT_UNAUTHORIZED'))) throw new Error('Unsafe donor transport retained');
const notices = JSON.parse(await fs.readFile(path.join(root, 'licenses/BUNDLED-PACKAGES.json'), 'utf8'));
const contributing = new Set(Object.entries(result.inputs).filter(([file, info]) => file.startsWith('node_modules/') && info.bytesInOutput > 0).map(([file]) => {
  const parts = file.split('/'), index = parts.lastIndexOf('node_modules');
  return parts.slice(0, index + (parts[index + 1].startsWith('@') ? 3 : 2)).join('/');
}));
if (contributing.size !== notices.packages.length || notices.packages.some(item => !contributing.has(item.package_path))) throw new Error('Bundled notice inventory mismatch');
for (const item of notices.packages) {
  for (const file of [`${item.package_path}/package.json`, item.retained_package_json]) {
    if (digest(await fs.readFile(path.join(root, file))) !== item.package_json_sha256) throw new Error(`Package notice binding mismatch: ${file}`);
  }
  for (const notice of item.notices) {
    if (digest(await fs.readFile(path.join(root, notice.path))) !== notice.sha256) throw new Error(`License notice mismatch: ${notice.path}`);
  }
}
await fs.copyFile(path.join(root, 'src/donor-factory.d.ts'), path.join(root, 'dist/donor-factory.d.ts'));
// npm omits package-lock.json from tarballs even when the files list names it.
await fs.copyFile(path.join(root, 'package-lock.json'), path.join(root, 'dist/BUILD-DEPENDENCY-LOCK.json'));
await fs.writeFile(path.join(root, 'dist/BUILD-MANIFEST.json'), JSON.stringify({
  schema_version: 1, upstream_revision: manifest.revision,
  source_manifest_sha256: digest(await fs.readFile(path.join(root, 'vendor/SOURCE-MANIFEST.json'))),
  package_lock_sha256: digest(await fs.readFile(path.join(root, 'package-lock.json'))),
  bundle_sha256: digest(bundle), bundle_bytes: bundle.length,
  build_dependencies: packageInfo.devDependencies,
  runtime_dependencies: packageInfo.dependencies,
  modifications: 'Exact retained donor source; ten auth/helper statement projections; per-instance real SDK transport injection; explicit Supabase admin constructor and Webflow v2 route ports; HubSpot Date result projection; Sheets literal-title and Graph ID input projections; Teams zero-retry helper composition; selected Monday Node/fetch/warning and Mailchimp instance/HTTP/data-center ports; S3 static-credential real SDK requestHandler and XML-model validation; Slack real WebClient adapter and Datadog explicit Configuration HTTP ports; exact Linear embedded fetch-binding projection; fixed selected read/write effect contracts; explicit result projections; generated per-invocation factory and common HTTP owner. See SOURCE-PROJECTIONS.json and THIRD_PARTY_NOTICES.md.',
  metafile: output.metafile,
}, null, 2) + '\n');
