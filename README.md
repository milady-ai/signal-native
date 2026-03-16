# @milady/signal-native

Native Node.js Signal client 
Wraps the [Presage](https://github.com/whisperfish/presage) Rust library with [napi-rs](https://napi.rs) bindings, providing a zero-dependency Signal messaging client for Node.js. No Docker, no Java, no signal-cli — just install and use.

## Features

- **Device linking** — Link as a secondary device via QR code (like Signal Desktop)
- **Registration** — Register as a primary device with SMS/voice verification
- **Send/receive messages** — Text messages to individuals and groups
- **Contacts & groups** — List contacts and groups from the Signal store
- **Reactions** — Send emoji reactions to messages
- **Attachments** — Incoming attachment metadata (download support planned)
- **Cross-platform** — Prebuilt binaries for macOS (Intel/ARM), Linux (x64), Windows (x64)

## Install

```bash
npm install @milady/signal-native
# or
bun add @milady/signal-native
```

## Quick Start

```javascript
import {
  linkDevice,
  finishLink,
  receiveMessages,
  sendMessage,
  getProfile,
} from "@milady/signal-native";

// 1. Link as secondary device
const provisioningUrl = await linkDevice("./signal-data", "My Bot");
console.log("Scan this QR code on your phone:", provisioningUrl);

// 2. Wait for link completion (user scans QR)
await finishLink("./signal-data");

// 3. Check profile
const profile = await getProfile("./signal-data");
console.log("Linked as:", profile.phone_number);

// 4. Receive messages
await receiveMessages("./signal-data", (message) => {
  console.log(`${message.sender_uuid}: ${message.text}`);

  // Reply
  sendMessage("./signal-data", message.sender_uuid, "Hello from Node.js!");
});
```

## API

### Device Management

- `linkDevice(dataPath, deviceName)` → `Promise<string>` — Returns provisioning URL for QR code
- `finishLink(dataPath)` → `Promise<void>` — Awaits link completion
- `register(dataPath, phoneNumber, useVoice)` → `Promise<void>` — Register as primary device
- `confirmRegistration(dataPath, code)` → `Promise<void>` — Confirm SMS/voice code
- `getProfile(dataPath)` → `Promise<Profile>` — Get own profile info

### Messaging

- `sendMessage(dataPath, recipientUuid, text)` → `Promise<{timestamp}>` — Send text to individual
- `sendGroupMessage(dataPath, groupIdB64, text)` → `Promise<{timestamp}>` — Send text to group
- `receiveMessages(dataPath, callback)` → `Promise<void>` — Stream incoming messages
- `stopReceiving(dataPath)` → `Promise<void>` — Stop the receive loop

### Contacts & Groups

- `listContacts(dataPath)` → `Promise<Contact[]>` — List stored contacts
- `listGroups(dataPath)` → `Promise<Group[]>` — List groups

### Reactions

- `sendReaction(dataPath, recipientUuid, emoji, targetTimestamp)` → `Promise<void>`

## Building from Source

Requires Rust toolchain and protoc:

```bash
# Install dependencies
cargo install napi-cli
npm install

# Build
napi build --platform --release
```

## License

AGPL-3.0-only (inherits from Presage)
