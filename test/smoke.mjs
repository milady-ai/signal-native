/**
 * Smoke test — verifies native module loads and all expected functions are exported.
 */

import { createRequire } from "node:module";
const require = createRequire(import.meta.url);

const native = require("../index.js");

const EXPECTED_EXPORTS = [
  "linkDevice",
  "finishLink",
  "register",
  "confirmRegistration",
  "sendMessage",
  "sendGroupMessage",
  "receiveMessages",
  "stopReceiving",
  "listContacts",
  "listGroups",
  "sendReaction",
  "getProfile",
];

let passed = 0;
let failed = 0;

for (const name of EXPECTED_EXPORTS) {
  if (typeof native[name] === "function") {
    passed++;
  } else {
    console.error(`FAIL: ${name} is not a function (got ${typeof native[name]})`);
    failed++;
  }
}

console.log(`\n${passed}/${EXPECTED_EXPORTS.length} exports OK`);

if (failed > 0) {
  console.error(`${failed} exports MISSING`);
  process.exit(1);
} else {
  console.log("Smoke test passed.");
}
