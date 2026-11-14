# ADR-0016: PluryFact reaches Pluryprint through a Chrome Native Messaging host

## Status

Accepted. The integration is **in development**: it ships with Pluryprint 0.0.6 and PluryFact 0.2, and nothing is announced as available before both are released.

## Date

2026-11-14

## Context

[PluryFact](https://pluryfact.maecly.com/) is a Chrome extension (Manifest V3) that downloads electronic vouchers (*comprobantes electrónicos*) from Ecuador's tax authority (SRI): for each voucher, the authorised **XML**, the printed representation (**RIDE**, a PDF) and a **TXT** report of the download, packed into a ZIP. Its users are accountants and small businesses who then print the RIDEs, often hundreds per month. Today they unzip the file and print it by hand.

What constrains the design:

- **PluryFact never knows where the ZIP is.** It builds the ZIP in memory and saves it with an `<a download>` from the SRI page's content script; it has no `downloads` permission. The bytes of every document are, however, in memory before they are zipped.
- **PluryFact 0.1.0 is under Chrome Web Store review.** A new *required* permission in an update disables the extension until each user accepts it and restarts the review; an *optional* permission requested at the moment of use does neither.
- **Pluryprint already has one way in from another process**: the handoff file of the context-menu integrations (`cli::handoff`), read by `pluryprint --invoke=context-menu --files-from <file>`, validated (schema version, freshness, containment in `<data dir>/handoff`, size caps), forwarded to a running window by single-instance and acted on by the same `applySelection` code path as the Explorer and Finder menus.
- **Pluryprint's security model has no listening socket** and one network destination of its own (`github.com`, for updates). Printing is a physical action with a cost; any channel that lets a web page or another process start a print is a liability.
- **Supported platforms for the integration are Windows and macOS.** Linux builds exist ([ADR-0015](./0015-linux-support.md)) but the integration is not promised there.

## Decision

PluryFact talks to Pluryprint through **Chrome Native Messaging**, with a host that ships inside Pluryprint.

- **Host.** A new console binary of the app crate, **`pluryprint-bridge`** (`src/bin/pluryprint-bridge.rs`, logic in `src/bridge/`), installed next to the app by every bundle: `%LOCALAPPDATA%\Pluryprint\pluryprint-bridge.exe`, `Pluryprint.app/Contents/MacOS/pluryprint-bridge`, `/usr/bin/pluryprint-bridge`. Host name **`com.maecly.pluryprint`**.
- **Who may start it.** The manifest's `allowed_origins` lists only PluryFact's Chrome Web Store origin, `chrome-extension://ibgodfpcihlddbbfaoooehfgbmahipbj/`. A development build of the extension is added explicitly (`pluryprint-cli bridge install --extra-origin chrome-extension://<id>/`). The host checks the origin the browser passes as its first argument against the manifests it is registered with, as defence in depth.
- **What travels.** The **bytes** of each document, base64-encoded, in chunks of at most 16 MiB, with the whole document's SHA-256. Never a path: the extension sends none and the host reads none it did not write. The ZIP itself is never sent nor unpacked in Rust; PluryFact unpacks it in the browser when the user prints an old download.
- **Which documents.** All three kinds PluryFact produces: RIDE PDFs (must start with `%PDF-`), voucher **XML** (must be well-formed UTF-8 without a DTD; Pluryprint learns to print it as a RIDE-like representation in its own change, ADR-0017 on the `feat/print-sri-xml` branch) and the TXT report (UTF-8 or ISO-8859-1 text).
- **Protocol v1.** JSON messages `{v, id, type, …}` answered by `{v, id, type, ok, …}` or `{…, ok: false, error: {code, message}}`: `hello`, `listPrinters`, `listProfiles`, `beginJob` (source, label, action, profile, printer, overrides, count), `putFile` (chunks), `commitJob`, `abortJob`; thirteen error codes. The canonical exchanges are fixtures in `src-tauri/tests/fixtures/bridge-protocol/`, copied byte for byte into PluryFact. See [ipc-and-cli.md §6](../04-architecture/ipc-and-cli.md#6-browser-bridge-native-messaging).
- **Where documents land.** `<data dir>/inbox/<jobId>/NNNN-<sanitized name>.<kind>`, in print order. A job is deleted if the port closes before `commitJob`; a committed job stays until the app's startup sweep finds it **older than 7 days**.
- **Delivery to the app.** On `commitJob` the host writes a schema-1 handoff (`source: "pluryfact"`, plus two optional, additive fields: a display `label` and the print `overrides`) and starts the app **detached** with `--invoke=context-menu --files-from <handoff> --action <a> [--profile <id>]`: in a process group of its own on macOS/Linux (the browser signals the host's group when the port closes), with `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP` (and breakaway from the browser's job object when allowed) on Windows. A running window receives it through single-instance (a named pipe on Windows, a Unix socket on macOS).
- **Default action `configure`.** The Print Job screen opens with the documents, PluryFact's settings pre-filled and a "From PluryFact" badge; nothing is spent until the user confirms. `queue` holds the batch in the queue; `print-default` prints at once and is only offered when the user chose it in PluryFact's settings (which asks again above 50 documents).
- **Overrides** are the vocabulary of `pluryprint print`'s flags (`copies` 1–999, `duplex`, `color`, `paper`, `orientation`, `collate`) plus `printerId`, mapped onto the existing per-job settings; an unknown override is refused, never ignored.
- **Registration.** Windows: the NSIS hooks write the manifest next to the bridge and `HKCU\Software\{Google\Chrome,Microsoft\Edge,Chromium}\NativeMessagingHosts\com.maecly.pluryprint` (Brave reads Chrome's key); the uninstaller removes them. macOS (no installer): the app registers at every start, idempotently, for each of Chrome, Edge, Brave and Chromium whose support folder exists, rewriting a manifest that points at another copy, and never from a translocated copy. Linux: `pluryprint bridge install`. Settings has a **Browser integration** section with the state per browser and **Reconnect**.
- **PluryFact side.** `nativeMessaging` is an **optional** permission, requested when the user clicks "Connect with PluryPrint". `connectNative` runs in the service worker only.

## Consequences

**Good.**

- No port, no server, no new network destination: the only channel is the pipe the browser opens to a process it starts, and only for an origin listed in a manifest the user's own install wrote.
- The whole app-side path is the existing one: handoff validation, single-instance forwarding, `applySelection`, the queue. The bridge adds a writer of handoff files, not a new way into the app.
- Large batches work: 5 000 documents, 50 MiB each, 1 GiB per job; a RIDE is 20–300 KB.
- Every document is checked (size, hash, kind) before the app sees it; names are reduced to safe base names; the inbox cannot be escaped.

**Bad.**

- Documents are written to disk (the inbox) and kept up to a week, so they can be printed from the queue later. They are as private as the rest of the user's data folder, not encrypted.
- macOS registration needs one launch of the app after it is installed (or moved). A user who installs PluryFact first sees "PluryPrint is not installed" until then; PluryFact says to open Pluryprint once.
- Unsigned builds: Gatekeeper's quarantine on macOS and SmartScreen on Windows may get in the way of the browser starting the bridge of a copy the user never opened. Not verified yet (no signing certificates, [ADR-0011](./0011-release-channel.md)).
- Managed Windows machines with `NativeMessagingUserLevelHosts` disabled ignore HKCU registrations. Documented; a per-machine install is out of scope.
- No status channel back: PluryFact learns that the app has the batch (`handed-off`), not how printing went. A `jobStatus` message with a correlation id is the next step.

## Alternatives Considered

### (b) A `pluryprint://` URL scheme (deep link)

Any web page can open a URL scheme, so it could never print directly, only open `configure`. The browser asks "Open Pluryprint?" on every use. It carries a short URL, not documents, so the files would already have to be on disk, which needs the `downloads` permission and a known folder. It has no reply channel: no printer or profile list, no confirmation. On macOS it cannot be registered at run time. **Rejected** as a transport; at most a future "open Pluryprint" shortcut.

### (c) An HTTP or WebSocket server on localhost

The model of QZ Tray and Zotero's connector. It would add a permanently listening port to an app whose security model has none, needs a pairing token, strict `Origin` checks and Host validation against DNS rebinding, firewall prompts on Windows, port collisions, and since Chrome 142 a Local Network Access permission prompt (or `host_permissions` on `http://127.0.0.1/*`, a new reviewed permission in PluryFact). Any local process could try to submit print jobs. **Rejected**: the worst security of the four for no capability Native Messaging lacks here.

### (d) A watched folder (the extension downloads into a folder Pluryprint watches)

An extension can only write under the user's Downloads folder, with the `downloads` permission (another prompt). Watching it is fragile (`.crdownload` partials, a custom Downloads folder, renamed duplicates) and turns any downloaded file with the right name into something Pluryprint prints. No reply channel either. **Rejected**; the plain ZIP download remains the fallback that needs no app at all.

### Native Messaging with paths instead of bytes, or the ZIP

Sending the ZIP's path needs the `downloads` permission and makes Pluryprint unpack an untrusted archive (zip-slip, zip bombs, and a crate with several 2026 advisories). Sending individual files' paths means hundreds of downloads first. Bytes are simpler and safer: the extension already has them in memory.

### `print --json` headless instead of the handoff

The headless CLI prints synchronously and would give PluryFact a per-file result, but it bypasses the queue and the window, cannot confirm with the user, and races a running app. One delivery path (the handoff) keeps one behaviour.

### `pluryprint-cli` as the host

It treats an unknown first argument (`chrome-extension://…/`) as a usage error (exit 2). A mode switch on that prefix would mix two very different contracts in one binary; a separate binary keeps the CLI's exit codes untouched and the host's stdout free of anything but frames.

## Operations matrix (summary)

The full matrix, every PluryFact operation against every Pluryprint operation, is in PluryFact's `docs/INTEGRACION-PLURYPRINT.md`. Pluryprint's side of it:

| Pluryprint operation | With PluryFact | Exposed through | Phase |
| --- | --- | --- | --- |
| `configure` / `queue` / `print-default` | yes, chosen in PluryFact's settings (`configure` by default) | `beginJob.action` → handoff `action` | MVP |
| Profiles | yes, default or a chosen one | `listProfiles`, `beginJob.profileId` | MVP |
| Printers | yes, profile's or a chosen one | `listPrinters`, `beginJob.printerId` | MVP |
| Copies, duplex, colour, paper, orientation, collate | yes, optional overrides | `beginJob.overrides` → Print Job pre-fill / batch settings | MVP |
| Page range, scaling | no: per document, not per batch of vouchers | Print Job screen after `configure` | discarded for the protocol |
| Print order | the order PluryFact sends (`index`) | file names `NNNN-…` | MVP |
| PDF (RIDE), TXT report | printed as today | `putFile.kind` `pdf` / `txt` | MVP |
| Voucher XML | printed as a RIDE-like representation | `putFile.kind` `xml` + ADR-0017 converter | MVP (with ADR-0017) |
| Queue: state, pause, cancel, retry | in Pluryprint's Queue screen | the app | MVP |
| Status back in PluryFact | no reply after `handed-off` | future `jobStatus` with a correlation id | next |
| `print --json`, context menus, Open with, drag and drop | unchanged; XML printing also benefits them | CLI / shell | MVP (ADR-0017) |
| Updates | unchanged; the bridge updates with the app | updater | MVP |
| Reprint from PluryFact's history | PluryFact unpacks an old ZIP in the browser and sends it again | protocol as above | MVP |
| A separator page per period | not built | — | next |
