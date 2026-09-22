const { test, chromium } = require("@playwright/test");
const { runSmoke, TEST_TIMEOUT_MS } = require("./smoke.cjs");

test("webrtc connectivity smoke", async () => {
  test.setTimeout(TEST_TIMEOUT_MS);
  await runSmoke(chromium);
});
