import assert from "node:assert/strict";
import test from "node:test";

import { resolveConsoleReadOnlyOverride } from "./read-only-override";

test("console read-only override accepts ACL-friendly query flags", () => {
  assert.equal(
    resolveConsoleReadOnlyOverride({ search: "?console_read_only=true" }),
    true,
  );
  assert.equal(
    resolveConsoleReadOnlyOverride({ search: "?meta_console_read_only=1" }),
    true,
  );
  assert.equal(
    resolveConsoleReadOnlyOverride({ search: "?view_only=yes" }),
    true,
  );
});

test("console read-only override can be provided by a host global", () => {
  assert.equal(resolveConsoleReadOnlyOverride({ hostOverride: true }), true);
  assert.equal(
    resolveConsoleReadOnlyOverride({ hostOverride: "true" }),
    true,
  );
});

test("console read-only override defaults writable and ignores false flags", () => {
  assert.equal(resolveConsoleReadOnlyOverride({ search: "" }), false);
  assert.equal(
    resolveConsoleReadOnlyOverride({
      search: "?console_read_only=false",
    }),
    false,
  );
  assert.equal(
    resolveConsoleReadOnlyOverride({
      search: "?console_read_only=false",
      hostOverride: true,
    }),
    true,
  );
  assert.equal(
    resolveConsoleReadOnlyOverride({
      search: "?console_read_only=true",
      hostOverride: false,
    }),
    true,
  );
});
