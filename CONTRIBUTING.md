# Contributing to Pluryprint

Contributions are welcome: bug reports, fixes, translations, ideas.

## Before you open a pull request

- **Read and agree to the [Contributor License Agreement](CLA.md).** You keep
  the copyright in what you write; the agreement lets the project's owner keep
  publishing Pluryprint, change its licence or offer it commercially later.
  The pull request template asks you to confirm it.
- **Run the checks.** `pnpm check-all` runs lint, formatting, type checks and
  the unit tests; the Rust side is `cargo test` and
  `cargo clippy --all-targets -- -D warnings` in `src-tauri/` (and in
  `shell-ext-win/` if you touched it).
- **Keep it small and say why.** One change per pull request, in
  [Conventional Commits](https://www.conventionalcommits.org/) form
  (`fix(queue): …`, `feat(ui): …`).

The full contribution guide lives in [`docs/08-development/contributing.md`](./docs/08-development/contributing.md). It covers:

- Local development setup
- Coding standards (Rust + TypeScript)
- Testing strategy
- The pull request workflow
- Code of conduct expectations

For a high-level orientation, also see:

- [`docs/08-development/local-setup.md`](./docs/08-development/local-setup.md) — prerequisites on Windows, macOS and Linux (the WebKitGTK and GTK development packages) and the first-run checklist
- [`docs/08-development/repo-structure.md`](./docs/08-development/repo-structure.md) — where things live
- [`docs/08-development/coding-standards.md`](./docs/08-development/coding-standards.md) — formatting + style

## Licence and name

Pluryprint is published under the [GNU Affero General Public License v3.0](LICENSE).
The name, the wordmark and the mark are not covered by that licence: a fork
must use its own (see [TRADEMARKS.md](TRADEMARKS.md)). For a commercial
licence or any other arrangement, write to hola@maecly.com.
