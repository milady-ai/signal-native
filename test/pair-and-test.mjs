/**
 * Interactive test: pair a Signal device via QR code, then send/receive messages.
 *
 * Usage:
 *   node test/pair-and-test.mjs                  # Link device + test
 *   node test/pair-and-test.mjs --skip-pair       # Skip pairing (already linked)
 *   node test/pair-and-test.mjs --send <uuid> "Hello"  # Send a message
 *
 * Auth data stored at ~/.milady/signal-test/signal.db
 */

import { createRequire } from "node:module";
import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);

// Load the native .node binary directly (includes testStoreOpen)
const __dirname = path.dirname(fileURLToPath(import.meta.url));
const nodePath = path.join(__dirname, "..", "signal-native.win32-x64-msvc.node");
let native;
try {
  native = require(nodePath);
} catch {
  // Fallback to index.js
  native = require("../index.js");
}

// Use a .db file path (not directory) — required by presage-store-sqlite
const AUTH_DB = path.join(os.homedir(), ".milady", "signal-test", "signal.db");

const args = process.argv.slice(2);
const skipPair = args.includes("--skip-pair");
const sendIdx = args.indexOf("--send");

// ---------------------------------------------------------------------------
// Step 1: Device pairing
// ---------------------------------------------------------------------------

async function pairDevice() {
  console.log("\n=== Step 1: Device Pairing ===\n");

  const dir = path.dirname(AUTH_DB);
  fs.mkdirSync(dir, { recursive: true });

  if (fs.existsSync(AUTH_DB)) {
    console.log(`Auth database exists: ${AUTH_DB}`);
    try {
      const profile = await native.getProfile(AUTH_DB);
      console.log(`Already linked as: ${profile.uuid}`);
      if (profile.phoneNumber) console.log(`Phone: ${profile.phoneNumber}`);
      return true;
    } catch {
      console.log("Auth data exists but profile check failed. Re-linking...");
      // Delete old db files
      for (const f of fs.readdirSync(dir)) {
        if (f.startsWith("signal.db")) fs.unlinkSync(path.join(dir, f));
      }
    }
  }

  console.log("Starting device linking...");
  console.log("You will need to scan a QR code with your Signal app.\n");

  try {
    const provisioningUrl = await native.linkDevice(AUTH_DB, "Milady AI");

    // Generate QR in terminal
    try {
      const qrcode = await import("qrcode-terminal");
      console.log("Scan with Signal → Settings → Linked Devices → +\n");
      qrcode.default.generate(provisioningUrl, { small: true });
    } catch {
      console.log("Provisioning URL (encode as QR code):\n");
      console.log(provisioningUrl);
      console.log("\n(Install qrcode-terminal for inline QR: bun add -d qrcode-terminal)");
    }

    console.log("\nWaiting for you to scan the QR code...");
    await native.finishLink(AUTH_DB);

    const profile = await native.getProfile(AUTH_DB);
    console.log(`\nLinked successfully as: ${profile.uuid}`);
    if (profile.phoneNumber) console.log(`Phone: ${profile.phoneNumber}`);
    return true;
  } catch (err) {
    console.error("Pairing failed:", err.message || err);
    return false;
  }
}

// ---------------------------------------------------------------------------
// Step 2: Send a test message
// ---------------------------------------------------------------------------

async function sendTestMessage(recipient, text) {
  console.log(`\n=== Sending message to ${recipient} ===\n`);
  try {
    const result = await native.sendMessage(AUTH_DB, recipient, text);
    console.log("Sent! Timestamp:", result?.timestamp);
    return true;
  } catch (err) {
    console.error("Send failed:", err.message || err);
    return false;
  }
}

// ---------------------------------------------------------------------------
// Step 3: Receive messages
// ---------------------------------------------------------------------------

async function receiveMessages() {
  console.log("\n=== Listening for incoming messages (Ctrl+C to stop) ===\n");

  try {
    await native.receiveMessages(AUTH_DB, (msg) => {
      if (msg.isQueueEmpty) {
        console.log("[sync complete — queue empty]");
        return;
      }

      const time = new Date(msg.timestamp).toLocaleTimeString();
      const sender = msg.senderUuid || "unknown";
      const group = msg.groupId ? ` [group: ${msg.groupId}]` : "";

      if (msg.isReaction) {
        console.log(`[${time}] ${sender}${group} reacted: ${msg.reactionEmoji}`);
      } else if (msg.text) {
        console.log(`[${time}] ${sender}${group}: ${msg.text}`);
      } else {
        console.log(`[${time}] ${sender}${group}: (non-text message, ${msg.attachments?.length || 0} attachments)`);
      }
    });
  } catch (err) {
    console.error("Receive error:", err.message || err);
  }
}

// ---------------------------------------------------------------------------
// Step 4: List contacts/groups
// ---------------------------------------------------------------------------

async function listInfo() {
  console.log("\n=== Contacts ===\n");
  try {
    const contacts = await native.listContacts(AUTH_DB);
    if (contacts.length === 0) {
      console.log("(no contacts synced yet)");
    } else {
      for (const c of contacts.slice(0, 20)) {
        const name = c.name || "(unnamed)";
        console.log(`  ${name} — ${c.uuid}${c.phoneNumber ? ` (${c.phoneNumber})` : ""}`);
      }
      if (contacts.length > 20) console.log(`  ... and ${contacts.length - 20} more`);
    }
  } catch (err) {
    console.log(`(contacts not available: ${err.message})`);
  }

  console.log("\n=== Groups ===\n");
  try {
    const groups = await native.listGroups(AUTH_DB);
    if (groups.length === 0) {
      console.log("(no groups synced yet)");
    } else {
      for (const g of groups.slice(0, 20)) {
        console.log(`  ${g.name} — ${g.id} (${g.membersCount || 0} members)`);
      }
      if (groups.length > 20) console.log(`  ... and ${groups.length - 20} more`);
    }
  } catch (err) {
    console.log(`(groups not available: ${err.message})`);
  }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

async function main() {
  console.log("Signal Native Test Tool");
  console.log("=======================\n");
  console.log(`Auth database: ${AUTH_DB}`);

  // Handle --send mode
  if (sendIdx !== -1) {
    const recipient = args[sendIdx + 1];
    const text = args[sendIdx + 2] || `Test from milady at ${new Date().toISOString()}`;
    if (!recipient) {
      console.error("Usage: --send <uuid-or-phone> [message]");
      process.exit(1);
    }
    await sendTestMessage(recipient, text);
    process.exit(0);
  }

  // Pair device
  if (!skipPair) {
    const paired = await pairDevice();
    if (!paired) {
      console.error("\nDevice pairing failed. Exiting.");
      process.exit(1);
    }
  } else {
    console.log("Skipping pairing (--skip-pair)");
  }

  // Show contacts/groups
  await listInfo();

  // Listen for messages
  console.log("\nStarting message listener...");
  console.log("Send a message to this device from another Signal client to test.\n");

  // Handle Ctrl+C gracefully
  process.on("SIGINT", async () => {
    console.log("\n\nStopping receiver...");
    try {
      await native.stopReceiving(AUTH_DB);
    } catch { /* ignore */ }
    console.log("Done.");
    process.exit(0);
  });

  await receiveMessages();
}

main().catch((err) => {
  console.error("Fatal error:", err);
  process.exit(1);
});
