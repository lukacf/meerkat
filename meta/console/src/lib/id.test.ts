import assert from "node:assert/strict";
import test from "node:test";

import { createConsoleId } from "./id";

test("createConsoleId uses randomUUID when available", () => {
  const id = createConsoleId("panel", {
    randomUUID: () => "11111111-2222-4333-8444-555555555555",
  });

  assert.equal(id, "panel-11111111-2222-4333-8444-555555555555");
});

test("createConsoleId falls back to getRandomValues when randomUUID is unavailable", () => {
  let next = 0;
  const id = createConsoleId("panel", {
    getRandomValues: (array) => {
      const bytes = array as Uint8Array;
      for (let i = 0; i < bytes.length; i += 1) {
        bytes[i] = next;
        next = (next + 1) % 256;
      }
      return array;
    },
  });

  assert.match(
    id,
    /^panel-[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
  );
});
