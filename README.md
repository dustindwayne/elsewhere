# Elsewhere

Desktop application for Windows, Linux, and macOS with a Rust backend and an optional paired phone web surface.

## Source layout

- `core/`: SQLite records, revision history, files, templates, agent access, peer exchange, and HTTP backend.
- `src-tauri/`: desktop window and platform integration.
- `web/`: bundled interface and responsive phone dashboard.
- `tests/`: spatial grouping and update sequencing tests.

## Development

Requires Rust, Node.js, and the platform dependencies for Tauri 2.

```sh
npm ci
npm run tauri -- dev
```

The desktop window opens the bundled interface. A loopback backend starts in the same process. Devices & data can enable a paired phone connection on the local network. Closing the desktop application stops its backend. Phone connection links contain a per-run access token; sessions use HttpOnly, SameSite cookies.

## Verification

```sh
npm test
```

## Local desktop build

```sh
npm run tauri -- build --no-bundle
```

Desktop packages must be built on their target operating systems. The Android source is an additional standalone client; it is not required for phone browser access.

## Runtime behavior

Text-pattern affinity, explicit connections, and user-defined categories form spatial groups. Groups retain regional anchors. Movement settles, and remote changes animate in sequence. The current affinity calculation uses local word patterns rather than model embeddings.

Data remains in the platform application-data directory. SQLite is not encrypted at rest. Peer exchange uses authenticated encrypted packets; the local phone dashboard uses HTTP session authentication. Agent access is separately granted through a loopback MCP endpoint.

No CI build workflow is included.
