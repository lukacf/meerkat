const path = require("node:path");

const distEntry = path.join(__dirname, "dist", "index.cjs");

try {
  module.exports = require(distEntry);
} catch (error) {
  if (error && error.code === "MODULE_NOT_FOUND") {
    throw new Error(
      "missing console/dist/index.cjs; run `npm run build` in console/ before importing @meerkat/console"
    );
  }
  throw error;
}
