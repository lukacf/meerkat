#!/usr/bin/env node
"use strict";
// Explicit live-provider acceptance lane; CI runs the deterministic real-runtime lane.
require("./scenario-registry.cjs").runScenarios(require("./scenarios/real-images.cjs").liveScenarios)
  .catch(error => { process.stderr.write(`${error.stack || error}\n`); process.exitCode = 1; });
