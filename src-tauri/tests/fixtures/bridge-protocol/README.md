# Bridge protocol v1 — canonical fixtures

One JSON file per message of the protocol between PluryFact and
`pluryprint-bridge` (`src-tauri/src/bridge/protocol.rs`,
`docs/04-architecture/ipc-and-cli.md` §6, ADR-0016):

- `<type>.request.json` / `<type>.response.json` — a successful exchange.
  `beginJob`, `putFile` (one PDF, one XML, one TXT document), `commitJob`
  form one job; `beginJob.queue`, `putFile.chunk-0` and `abortJob` a second,
  chunked one that is aborted.
- `error.<code>.response.json` — one reply per error code, and
  `error.<code>.request.json` where a single request produces it.

These files are the **source of truth** for both repositories. PluryFact
keeps a byte-for-byte copy in `test/fixtures/pluryprint-protocol/`; change
them here first, then copy them over. `src-tauri/tests/bridge_protocol_contract.rs`
checks that the Rust types read and write every file exactly and replays the
exchanges against the real state machine.

Every document in them is fictitious: RUC `0999999999001`, made-up access
key, a minimal PDF.
