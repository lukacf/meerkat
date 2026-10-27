"use strict";
// npm lifecycle execution is forbidden for this fixture. This marker is
// inspected after installation; the selected explicit compile does not run it.
require("node:fs").writeFileSync(require("node:path").join(__dirname, "LIFECYCLE-RAN"), "unexpected\n");
throw new Error("Unexpected npm lifecycle execution");
