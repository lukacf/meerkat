const { chromium } = require("@playwright/test");
const { runSmoke } = require("./smoke.cjs");

runSmoke(chromium).catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
