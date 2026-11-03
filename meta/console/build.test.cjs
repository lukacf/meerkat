const assert = require("node:assert/strict");
const test = require("node:test");
const vm = require("node:vm");
const { transform } = require("esbuild");
const { bundleSyntaxOptions } = require("./build.cjs");

// Whitespace is inside source strings here, including whitespace-only lines.
// A text formatter must not change the runtime contents of these templates.
const source = [
  'const middle = "value";',
  'const tag = (strings, ...values) => ({ cooked: [...strings], raw: [...strings.raw], values });',
  'globalThis.result = {',
  '  plain: `first  ',
  '        ',
  'last\\t`,',
  '  interpolated: `left  ',
  '  ${middle}\\t',
  'right  `,',
  '  raw: String.raw`escaped\\n  ',
  '    ',
  '\\x20`,',
  '  tagged: tag`start\\t  ',
  '  ${middle}',
  'end  `,',
  '  invalidEscape: tag`\\unicode ${middle}`,',
  '};',
].join("\n");

function evaluate(code) {
  const context = vm.createContext({});
  vm.runInContext(code, context);
  return JSON.parse(JSON.stringify(context.result));
}

for (const [format, minify] of [["cjs", false], ["iife", true]]) {
  test(`${format} lowering preserves template contents without trailing whitespace`, async () => {
    assert.match(source, /[\t ]+$/m, "fixture must include runtime-significant trailing whitespace");
    const options = { ...bundleSyntaxOptions, target: "es2020", format, minify };
    const first = await transform(source, options);
    const second = await transform(source, options);
    assert.deepEqual(evaluate(first.code), evaluate(source));
    assert.doesNotMatch(first.code, /[\t ]+$/m);
    assert.equal(first.code, second.code, "identical input must produce identical bytes");
  });
}
