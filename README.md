<p align="center">
  <img src="assets/branding/elsewhere-readme.png" alt="elsewhere. — externalized thought." width="100%">
</p>

<p align="center">
  <strong>A spatial home for thoughts, notes, and connections.</strong><br>
  Desktop application · Local storage · Optional phone access
</p>

<p align="center">
  <a href="#the-space">The space</a> &nbsp;·&nbsp;
  <a href="#your-devices">Your devices</a> &nbsp;·&nbsp;
  <a href="#agent-memory">Agent memory</a> &nbsp;·&nbsp;
  <a href="#build-from-source">Build from source</a>
</p>

---

## The space

Elsewhere gives ideas room to develop. Capture a thought, expand it into a note, or choose a reusable template when structure helps. Connections remain visible as the space grows.

Related thoughts gather into neighborhoods with stable anchors. Motion settles once the layout finds its balance. Changes arrive gradually, one at a time, while opening a thought pauses the movement for reading and editing.

![An example Elsewhere workspace with connected thoughts](assets/branding/workspace-preview.png)

<p align="center"><sub>Workspace preview with example content.</sub></p>

| Capture | Connect | Develop |
| --- | --- | --- |
| Add a thought, note, or template-based record. Changes save automatically. | Link ideas, follow relationships, and find related material in the space. | Add attachments, use checklists and fields, and revisit earlier revisions. |

The current layout uses local word-pattern similarity, explicit links, and optional categories. Model-based semantic grouping is not implemented.

## Your devices

The desktop app opens its own window and starts its Rust backend in the same process. The phone dashboard is another surface for that backend.

| Surface | Availability |
| --- | --- |
| **Windows desktop** | Local desktop build verified. |
| **macOS and Linux desktop** | Tauri source targets available; native packages have not been verified. |
| **Phone browser** | Responsive dashboard served by the desktop backend after pairing on the local network. |
| **Standalone Android** | Additional native client. ARM64 package builds locally; device testing remains outstanding. |

### Open the space on a phone

1. Open **Space options → Devices & data** in the desktop app.
2. Choose **Show phone connection**.
3. Scan the code or open the connection link on a phone on the same network.

Keep the desktop app open while using its phone dashboard. The connection link contains a per-run access token.

Independent native clients can also pair and exchange changes. Each retains a local copy, and competing revisions remain available for review.

## Agent memory

Each agent has a separate memory store. User knowledge remains in its own store. Access to another agent’s memory or user knowledge is denied until the user grants it.

| User knowledge | Agent memory |
| --- | --- |
| Thoughts and notes in the main space. | Persistent records owned by one named agent. |
| Used by the canvas, user search, backups, and peer exchange. | Stored in a separate encrypted vault and inspected through **Agent access**. |
| Accessible to agents only with an explicit user grant. | Exposed through a local MCP endpoint with individually revocable connections. |

In **Space options → Agent access**, create a named connection and choose read-only or read/add/edit access. Copy the generated endpoint and bearer token into a compatible MCP client. The token is displayed once. Expand **Shared memory permissions** on a connection to grant or revoke read or read-and-edit access to a specific store. Tools accept an optional `store` owner ID (`user` for user knowledge); omitting it selects the agent’s own store. MCP resources expose only the connection’s own store.

Available operations:

- Search agent memory and read individual records.
- Follow relationships and inspect revision history.
- Create and update memory records when write access is granted.
- Reject stale edits using revision checks and attribute changes to the connection.

The MCP endpoint binds to loopback. Hosted agents need a separate supported connection to reach it. Agent memory is excluded from user backups and peer exchange; a dedicated agent-memory backup flow is not implemented.

## Build from source

**Development preview · v0.2.0**

Install Rust, Node.js with npm, and the [Tauri platform prerequisites](https://v2.tauri.app/start/prerequisites/).

```sh
git clone https://github.com/dustindwayne/elsewhere.git
cd elsewhere
npm ci
npm run tauri -- dev
```

### Desktop

Build on the operating system you want to target:

```sh
npm run desktop:build
```

This builds the desktop application without an installer bundle. Compiled executables are written under `target/release/`.

### Android

Install the Android SDK, NDK, a compatible JDK, and the Rust Android target. Initialize the native project when required:

```sh
npm run android:init
npm run android:build
```

A Windows packaging helper is also available:

```powershell
./scripts/build-android.ps1
```

### Verify

```sh
npm test
```

The test suite covers encrypted migration and reopening, tamper and wrong-key rejection, cross-store permission boundaries, record revisions, attachments, authenticated phone sessions, peer exchange, spatial settling, and sequential visual updates. Windows key-store integration has a separate opt-in test; macOS, Linux, and Android runtime encryption still require device verification.

<details>
<summary><strong>Architecture and data</strong></summary>

### Source map

| Directory | Responsibility |
| --- | --- |
| `core/` | Rust storage, revisions, templates, files, agent memory, HTTP backend, and peer transport. |
| `src-tauri/` | Desktop window, platform integration, icons, and Android client source. |
| `web/` | Bundled interface, phone dashboard, spatial simulation, and transitions. |
| `tests/` | Spatial behavior and visual sequencing tests. |

### Storage boundaries

```text
Application data/
├── <owner-hash>.vault          Encrypted user knowledge and settings
├── memory.lock                Exclusive process lock, no memory content
└── agents/
    └── <agent-id>/
        └── <owner-hash>.vault  Encrypted memory for one agent
```

Records use an in-memory SQLite index persisted as an authenticated AES-256-GCM vault. Titles and bodies use raw UTF-8 with length framing; revision metadata uses MessagePack rather than JSON documents. MCP, import/export, and network messages retain their protocol formats. Attachments are deduplicated by content hash. Revision history preserves earlier content and conflicting edits. Each save atomically replaces the encrypted vault; this currently rewrites the complete store, so large stores require further performance work.

Templates are inert, versioned data. Installed template packages require a valid signature and supported element types.

### Network and encryption

- The backend starts on loopback. Phone access enables a local-network listener with session authentication.
- Phone sessions use HttpOnly, SameSite cookies over HTTP; this dashboard connection is not encrypted by TLS.
- Native peer exchanges use authenticated encrypted packets.
- Agent connections use separate bearer tokens on a loopback MCP listener.
- Persisted memory is always encrypted, including while the app is open. Keys live in the operating-system credential store, not alongside the vault. Windows uses Credential Manager; macOS uses Keychain; Linux requires an available Secret Service; Android uses its native Keystore-backed credential adapter.
- The backend opens user memory only while an operation holds access, then closes the decrypted database. An agent store is opened for a request and dropped afterward. The active interface necessarily holds the records it is displaying. Keys are fetched per encryption/decryption operation. This does not promise secure erasure of process memory, swap, or crash dumps, or automatic device-lock detection.
- Missing or inaccessible keys fail closed. There is no plaintext fallback. Vault files are bound to their workspace and owner; copying them alone to another device does not transfer access. A key recovery/export workflow is not implemented.
- Existing databases are checked, copied to verified encrypted storage, and their plaintext files removed. Legacy backups and the former shared agent store are preserved encrypted; they are not automatically assigned to an agent. Removing old files does not guarantee physical erasure from disks or previous backups.
- New drafts stay in application memory until the backend confirms an encrypted save. Browser local storage no longer receives new plaintext drafts; pending unsaved edits need a live backend. Existing browser drafts are copied to encrypted backend storage before their plaintext browser entries are removed. Drafts for missing records remain archived rather than being imported into the space. Deliberate JSON exports remain plaintext.

The backend development preview can be started with `npm run backend:dev`. The desktop application remains the primary executable.

No CI build workflow is included.

</details>
