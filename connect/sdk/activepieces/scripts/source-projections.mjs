import fs from 'node:fs/promises';
import path from 'node:path';
import ts from 'typescript';
import { createHash } from 'node:crypto';

// The selected actions import auth from piece indexes. Retain the exact auth
// declarations and their imports, without initializing the unrelated registry.
const plans = {
  'packages/pieces/community/freshservice/src/index.ts': { imports: ['@activepieces/pieces-framework', '@activepieces/pieces-common'], declarations: ['freshserviceAuth'] },
  'packages/pieces/community/linear/src/index.ts': { imports: ['@activepieces/pieces-framework', '@linear/sdk'], declarations: ['markdown', 'linearAuth'], functions: ['isUnauthorized'] },
  'packages/pieces/community/mailchimp/src/index.ts': { imports: ['@activepieces/pieces-framework'], declarations: ['mailchimpAuth'] },
  'packages/pieces/community/xero/src/index.ts': { imports: ['@activepieces/pieces-framework'], declarations: ['xeroAuth'] },
  'packages/pieces/community/freshdesk/src/index.ts': { imports: ['@activepieces/pieces-framework'], declarations: ['freshdeskAuth'] },
  'packages/pieces/community/webflow/src/index.ts': { imports: ['@activepieces/pieces-framework'], declarations: ['webflowAuth'] },
  'packages/pieces/community/toggl-track/src/index.ts': {
    imports: ['@activepieces/pieces-framework', '@activepieces/pieces-common'],
    declarations: ['validateAuth', 'togglTrackAuth'],
  },
  'packages/pieces/community/stripe/src/index.ts': {
    imports: ['@activepieces/pieces-framework', '@activepieces/pieces-common'],
    declarations: ['stripeAuth'],
  },
  'packages/pieces/community/wordpress/src/index.ts': {
    imports: ['@activepieces/pieces-framework', '@activepieces/pieces-common', './lib/common'],
    declarations: ['markdownPropertyDescription', 'wordpressAuth'],
  },
  'packages/pieces/common/src/lib/helpers/index.ts': {
    imports: [], declarations: ['getAccessTokenOrThrow'],
  },
};
const sha = text => createHash('sha256').update(text).digest('hex');

export async function sourceProjections(root) {
  const vendor = path.join(root, 'vendor/activepieces');
  const records = [];
  const projections = new Map();
  for (const [relative, plan] of Object.entries(plans)) {
    const file = path.join(vendor, relative);
    const text = await fs.readFile(file, 'utf8');
    const ast = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true);
    const seen = new Set();
    const slices = [];
    for (const statement of ast.statements) {
      const names = ts.isVariableStatement(statement) ? statement.declarationList.declarations.map(d => d.name.getText(ast)) : [];
      const keep = ts.isImportDeclaration(statement) ? plan.imports.includes(statement.moduleSpecifier.text) :
        names.length === 1 && plan.declarations.includes(names[0]) || ts.isFunctionDeclaration(statement) && plan.functions?.includes(statement.name?.text);
      if (!keep) continue;
      names.forEach(name => seen.add(name));
      const start = statement.getStart(ast), end = statement.end;
      slices.push({ start_utf16: start, end_utf16: end, start_line: ast.getLineAndCharacterOfPosition(start).line + 1,
        end_line: ast.getLineAndCharacterOfPosition(end).line + 1, text: text.slice(start, end) });
    }
    if (plan.declarations.some(name => !seen.has(name))) throw new Error(`Projection declaration missing: ${relative}`);
    const contents = slices.map(s => s.text).join('\n') + '\n';
    const output = path.join(root, 'dist/SOURCE-PROJECTIONS', relative);
    await fs.mkdir(path.dirname(output), { recursive: true });
    await fs.writeFile(output, contents);
    records.push({ source: relative, source_sha256: sha(text), projected_sha256: sha(contents),
      retained_declarations: plan.declarations, slices: slices.map(({ text, ...positions }) => ({ ...positions, sha256: sha(text) })) });
    projections.set(file, contents);
  }
  await fs.writeFile(path.join(root, 'dist/SOURCE-PROJECTIONS.json'), JSON.stringify({ schema_version: 1,
    policy: 'Exact top-level statement slices; selected run bodies and framework are unchanged. Unselected piece registrations are not evaluated.', records }, null, 2) + '\n');
  return { name: 'explicit-source-projections', setup(build) {
    build.onResolve({ filter: /helpers$/ }, args => {
      const original = path.join(vendor, 'packages/pieces/community/datadog/src/lib/common/helpers');
      if (path.resolve(path.dirname(args.importer), args.path) === original) return { path: path.join(root, 'bundle/datadog-config-port.ts') };
    });
    build.onResolve({filter:/^(node-fetch|.*monday-api-helpers)$/},args=>{
      if(args.importer.includes('/node_modules/monday-sdk-js/'))return {path:path.join(root,args.path==='node-fetch'?'bundle/monday-fetch-port.ts':'bundle/monday-warnings-port.ts')};
    });
    build.onResolve({filter:/^superagent$/},args=>args.importer.includes('/node_modules/@mailchimp/mailchimp_marketing/')?{path:path.join(root,'bundle/mailchimp-http-port.ts')}:undefined);
    build.onResolve({filter:/common$/},args=>{
      const original=path.resolve(path.dirname(args.importer),args.path);
      if(original===path.join(vendor,'packages/pieces/community/mailchimp/src/lib/common')&&args.importer!==path.join(root,'bundle/mailchimp-common-port.ts'))return {path:path.join(root,'bundle/mailchimp-common-port.ts')};
      if(original===path.join(vendor,'packages/pieces/community/amazon-s3/src/lib/common'))return {path:path.join(root,'bundle/s3-client-port.ts')};
    });

    build.onResolve({ filter: /client$/ }, args => {
      const original = path.join(vendor, 'packages/pieces/community/webflow/src/lib/common/client');
      if (args.importer !== path.join(root, 'bundle/webflow-client-port.ts') && path.resolve(path.dirname(args.importer), args.path) === original) {
        return { path: path.join(root, 'bundle/webflow-client-port.ts') };
      }
    });
    build.onResolve({ filter: /graph$/ }, args => {
      const original = path.join(vendor, 'packages/pieces/community/microsoft-teams/src/lib/common/graph');
      if (args.importer !== path.join(root, 'bundle/teams-graph-port.ts') && path.resolve(path.dirname(args.importer), args.path) === original) {
        return { path: path.join(root, 'bundle/teams-graph-port.ts') };
      }
    });
    build.onLoad({ filter: /\.ts$/ }, args => projections.has(args.path) ?
      { contents: projections.get(args.path), loader: 'ts', resolveDir: path.dirname(args.path) } : undefined);
  } };
}
