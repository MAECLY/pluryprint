# Contributing

> **TL;DR.** Open a discussion or issue before non-trivial work. Agree to the [Contributor License Agreement](../../CLA.md), use Conventional Commits, keep PRs focused. Look for `good first issue` labels to start.

Welcome. Pluryprint is open source under the [GNU AGPL v3.0](../09-legal/license.md), and we genuinely want help. This document is the social and procedural contract for contributing.

The repository's root `CONTRIBUTING.md` gives the short version (the CLA, the checks, the licence and the name) and points here; the full text lives in the docs tree so it can cross-link with other developer docs.

## Code of Conduct

By participating in this project — issues, pull requests, discussions, social channels — you agree to abide by the [Contributor Covenant 2.1](https://www.contributor-covenant.org/version/2/1/code_of_conduct/) as adopted in `CODE_OF_CONDUCT.md` at the repo root.

In short: be kind, assume good faith, and disagree without being personal. Maintainers will enforce the code; report violations privately to the email in `CODE_OF_CONDUCT.md`.

## Quick start for contributors

1. Read [local-setup.md](./local-setup.md) and get the project building locally.
2. Read [coding-standards.md](./coding-standards.md).
3. Find an issue labeled `good first issue` or `help wanted`, or open a discussion describing the change you want to make.
4. Fork the repo, create a feature branch (`feature/short-description`), make your change, ensure CI is green locally (`pnpm check-all`), open a PR.
5. Wait for review. Most PRs get a first response within 3-5 days; complex ones may take longer.

## Issue-first for non-trivial changes

For **trivial** changes (typo fixes, small doc edits, a one-line fix with an obvious test), open a PR directly.

For **non-trivial** changes (anything > ~100 LOC, new features, public API changes, dependency additions), **open an issue or a GitHub Discussion first**. The pattern:

1. Open an issue describing the problem and proposed solution.
2. Maintainer responds with a thumbs-up, a thumbs-down with reason, or questions.
3. Once agreed, write the code.
4. Open the PR linking to the issue.

This avoids the painful situation where you spend a weekend on a PR that the maintainer can't merge for reasons that could have been clarified in a 5-minute discussion.

## Issues

### Bug reports

Use the **Bug report** issue template. Include:

- Pluryprint version (`pluryprint --version` or in the About page).
- OS version (Windows 10 22H2, macOS 14.4, Ubuntu 24.04, etc.). On Linux also the package (AppImage or `.deb`), the desktop environment and whether the session is X11 or Wayland.
- Reproduction steps. Be specific. "It doesn't work" is not a reproduction.
- Expected vs actual behavior.
- Logs from `%LOCALAPPDATA%\Maecly\Pluryprint\data\logs\` (Windows), `~/Library/Logs/Pluryprint/` (macOS) or `~/.local/share/pluryprint/logs/` (Linux). Redact filenames if sensitive.
- Screenshots if it's a UI issue.

### Feature requests

Use the **Feature request** issue template. Describe:

- The problem you're trying to solve (not the solution you've already designed).
- Use cases. Who benefits and how often?
- Workarounds you've tried.
- Whether this fits the project's [vision](../01-vision/) (we focus on local batch printing; cloud features are out of scope).

### Security issues

**Do not open a public issue.** Email the address in `SECURITY.md` at the repo root. We respond within 72 hours and coordinate disclosure.

## Pull requests

### Before opening a PR

- The change has an associated issue (or you've checked it's trivial).
- Your branch is up-to-date with `main` (`git fetch origin && git rebase origin/main`).
- All tests pass locally (`pnpm check-all`).
- New code has tests (unit at minimum; integration where applicable).
- Documentation is updated for user-facing changes (CLI flags, GUI flows, profile schema).
- CHANGELOG.md has an entry under `## [Unreleased]` for user-visible changes.

### PR template

The repo's `PULL_REQUEST_TEMPLATE.md` is auto-loaded when you open a PR. The fields:

```markdown
## Description

What does this PR do?

## Motivation

Why is this change needed? Link the issue: Fixes #123 or Refs #123.

## Testing

How was this tested? List tests added/modified. For UI changes, include
screenshots or screen recordings.

## Checklist

- [ ] Tests added / updated
- [ ] Documentation updated (if user-facing)
- [ ] CHANGELOG.md updated (if user-visible)
- [ ] I have read the CLA and agree to it for this pull request
- [ ] Conventional Commits format
- [ ] CI green
```

Filling in this template is required. Don't delete fields; if a field doesn't apply, write "N/A — <reason>".

### PR title

The PR title becomes the squash-merged commit subject on `main`. Use Conventional Commits format:

- `feat(queue): add priority sorting`
- `fix(printer/windows): handle paper-out without panicking`
- `docs(roadmap): update M3 timeline`

See [coding-standards.md#commits](./coding-standards.md#commits) for details.

### PR size

Aim for PRs that one reviewer can fully understand in 30 minutes. As a rough guide:

| Lines changed | Difficulty to review | Verdict |
|---|---|---|
| < 50 | Trivial | Easy to land. |
| 50–300 | Normal | Most PRs. |
| 300–800 | Hard | Acceptable but consider splitting. |
| > 800 | Very hard | Strongly consider splitting. We may ask. |

Splitting a large change into a stack of small PRs (each independently mergeable) is appreciated and usually faster overall.

### Contributor License Agreement

Contributions come in under the [Contributor License Agreement](../../CLA.md) (CLA). You keep the copyright in what you write; the agreement gives the project's owner a licence to it, including the right to publish it under other terms later (another open-source licence, or a commercial one). That is what lets Pluryprint stay AGPL-3.0 today and still be offered on other terms if the owner chooses to.

You agree to it by opening a pull request: the pull request template has a checkbox for it, and a pull request without that confirmation is not merged. If you contribute on behalf of a company, or have a question about the agreement, write to hola@maecly.com before contributing.

There is no CLA bot and no sign-off check in CI. A `Signed-off-by:` line (`git commit -s`) is still welcome, but it is not what the project relies on; the CLA is.

### Conventional Commits

Each commit (or at least each squashed PR) must use [Conventional Commits](https://www.conventionalcommits.org/) format. See [coding-standards.md#commits](./coding-standards.md#commits) for the full ruleset.

Most importantly: subject ≤72 chars, lowercase after the colon, no trailing period, imperative mood.

### Review process

1. Open the PR. CI starts running.
2. CI must be green before review begins. If your PR is still in progress, mark it as a draft.
3. A maintainer reviews. Initially this is the project lead; over time we'll grow the reviewer pool.
4. Reviewer leaves comments. Address them by:
   - Making changes and pushing additional commits (do not force-push during active review unless asked; squash is automatic at merge).
   - Replying to comments inline when discussion is needed.
   - Resolving conversations once addressed.
5. Once approved and CI green, a maintainer squash-merges. The PR title becomes the commit subject on `main`.

### Review SLA

We aim for:

- First response within **3-5 days** for new PRs.
- Subsequent responses within **2-3 days**.
- Decision (merge or close) within **2 weeks** for standard PRs; longer for large or controversial ones.

If you haven't heard anything in over a week, ping the PR with a polite reminder. Maintainers are humans with day jobs; pings are welcome and not annoying.

### What gets your PR rejected

In rough order of likelihood:

1. **No issue / discussion first** for non-trivial changes.
2. **Out of scope.** New features that don't fit the project vision (e.g. cloud printing, OCR).
3. **Insufficient testing.** New code without new tests.
4. **CI red.** Self-explanatory.
5. **License-incompatible code.** Copy-pasting from source whose licence is not compatible with the AGPL-3.0.
6. **Breaking changes** without a SemVer-major bump justification.

We try to redirect rather than just close — most "rejected" PRs get reshaped into something acceptable through review.

## Branching

See [coding-standards.md#branches](./coding-standards.md#branches). The short version:

- `main` is the only long-lived branch.
- Work in `feature/<short-description>` or `fix/<short-description>` branches.
- Branch lifetime < 1 week. If your branch is older than a week, rebase on `main` and consider whether to split.
- Delete branches after merge.

## First good issues

Issues labeled `good first issue` are intentionally smaller in scope and well-described. They are the recommended entry point for first-time contributors.

We protect this label: maintainers tag an issue `good first issue` only after confirming:

- Scope is small (< 100 LOC change).
- Acceptance criteria are clear.
- Required context is documented or linked.
- The fix doesn't depend on understanding deeply unfamiliar parts of the codebase.

If you take a `good first issue`, comment "I'd like to take this" so others know it's claimed. If you start work and get stuck, comment again — we'd rather help you finish than have it sit idle.

## Translations

We welcome translations. The frontend uses `react-i18next` with files at `src/i18n/<lang>.json`.

To add a new language:

1. Open an issue: "Add <Language> translation".
2. Maintainer adds the language file scaffold and confirms.
3. Translate `src/i18n/en.json` keys to `src/i18n/<lang>.json`. Keep the JSON structure identical.
4. Update `src/i18n/i18n.ts` to register the language.
5. Open a PR.

We accept partial translations as long as English fallbacks are clear (which they are by default). We prefer machine-translated PRs to be reviewed by a native speaker before merge; if you can't find a reviewer, label the PR `translation-needs-review`.

To update an existing translation:

- Run `pnpm i18n:check` (locally) to find untranslated or stale keys.
- Translate or update them in the appropriate `<lang>.json` file.
- Open a PR.

We do **not** require the maintainer to be fluent in your language to merge; we trust translation contributors.

## Documentation contributions

Documentation lives in `docs/`. Contributing docs follows the same PR flow as code, but:

- The `docs:` Conventional Commit type applies.
- The CLA applies to documentation too.
- We prefer existing-doc edits over new files (avoid sprawl).
- Cross-links should use relative paths so they survive renames.

## Releases

Maintainers cut releases. Contributors **do not** push tags. The release process is documented in [release-checklist.md](./release-checklist.md).

If you have a fix that's particularly time-sensitive (security, regression), call it out in the PR; we'll prioritize cutting a patch release.

## Becoming a maintainer

Maintainership is earned, not assigned. The path:

1. Land 5+ substantive PRs (not typo fixes; meaningful changes).
2. Review others' PRs constructively.
3. Demonstrate a feel for the project's scope and culture.
4. A current maintainer nominates you; existing maintainers consensus.

Maintainer responsibilities: review PRs, triage issues, cut releases, enforce code of conduct.

We have no minimum activity requirement, but maintainers who go inactive for >6 months may be moved to "emeritus" status (still credited, no merge rights) to keep the active set tight.

## Recognition

Every contributor is listed in `CHANGELOG.md` for the release that included their change. Major contributions are also called out in release announcement blog posts.

There is no copyright assignment: you keep the copyright in your contributions and license them to the project's owner under the [CLA](../../CLA.md). See [license.md](../09-legal/license.md) for the full discussion.

## Communication channels

| Channel | Use for |
|---|---|
| GitHub Issues | Bugs, feature requests, structured discussions |
| GitHub Discussions | Open-ended questions, design conversations, community chat |
| Email (in `SECURITY.md`) | Security vulnerabilities |
| Project Discord/Matrix (when established) | Real-time chat, casual help |

We do not yet have a real-time chat. When usage warrants it (probably ~M3), we'll set one up.

## Thank you

Open source projects live and die by their contributors. Whether you're fixing a typo, translating a UI, or rewriting the print engine, your work matters and is appreciated.

## See also

- [coding-standards.md](./coding-standards.md)
- [local-setup.md](./local-setup.md)
- [testing-strategy.md](./testing-strategy.md)
- [repo-structure.md](./repo-structure.md)
- [../09-legal/license.md](../09-legal/license.md)
