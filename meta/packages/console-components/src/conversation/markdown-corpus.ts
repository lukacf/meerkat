/** Progressive prefixes and replacements are shared by correctness and profiling. */
export const MARKDOWN_CORPUS = [
  { id: "nested-fence", source: "- parent\n  - child\n\n    ```ts\n    const n = 1;\n    ```\n", selector: "ul ul pre code" },
  { id: "table", source: "| Left | Right |\n| :--- | ---: |\n| one | two |\n", selector: "table tbody td" },
  { id: "reference-after-use", source: "[reference][later]\n\n[later]: https://example.test \"Title\"\n", selector: "a" },
  { id: "blockquote", source: "> quoted\n>\n> - nested\n", selector: "blockquote ul" },
  { id: "escaped-markers", source: "\\*literal\\* and `a_b`", selector: "code" },
  { id: "incomplete-fence", source: "```rust\nfn main() {", selector: "pre code" },
  { id: "unicode-crlf", source: "# Živjo 世界\r\n\r\nCrème 🦦\r\n", selector: "h1" },
  { id: "gfm", source: "- [x] complete\n- [ ] next\n\n~~old~~ https://example.test\n", selector: 'input[type="checkbox"]' },
  { id: "footnote", source: "Evidence[^one].\n\n[^one]: Recorded result.\n", selector: 'a[data-footnote-ref]' },
  { id: "long-token", source: "word".repeat(1024), selector: "p" },
  { id: "json", source: ' {"message":"hello", "rows":[1,2]}\n', selector: "pre code.language-json" },
] as const;

export function markdownEditSequence(source: string): string[] {
  const cuts = new Set([0, 1, Math.floor(source.length / 3), Math.floor(source.length / 2), source.length - 1, source.length]);
  return [...cuts].filter((cut) => cut >= 0).sort((a, b) => a - b).map((cut) => source.slice(0, cut)).concat([
    source.replace(/\n/u, "\n\nreplacement\n"),
    source.slice(0, Math.floor(source.length / 2)),
    source,
  ]);
}
