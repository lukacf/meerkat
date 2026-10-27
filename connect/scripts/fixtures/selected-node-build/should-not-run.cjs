"use strict";
// Negative fixture: an installer that enables lifecycle scripts must fail.
require("node:fs").writeFileSync("lifecycle-script-ran", "unexpected execution\n", {flag: "wx"});
process.exit(99);
