"use strict";
// Consumer probe only. The installer must never execute this file.
const addon = require("./build/Release/addon.node");
const input = JSON.parse(process.argv[2]);
if (!input || Object.keys(input).sort().join(",") !== "left,right" ||
    !Number.isSafeInteger(input.left) || !Number.isSafeInteger(input.right)) {
  throw new Error("Expected exactly two safe integer operands");
}
const sum = addon.add(input.left, input.right);
if (!Number.isSafeInteger(sum)) throw new Error("Native sum is not a safe integer");
process.stdout.write(JSON.stringify({sum}) + "\n");
