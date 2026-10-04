# Contributing to Griffin

Thank you for your interest. Griffin is at an early stage: Phase 1 (web layer and LiveView) is built, nothing is released, and the API will move.

By taking part you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md).

## Where things are

| What | Where |
|---|---|
| What Griffin is and why | [README.md](README.md) |
| What is planned, in order | The [milestones](https://github.com/griffin-rs/griffin/milestones) |
| Specs and work tickets | [GitHub Issues](https://github.com/griffin-rs/griffin/issues) (specs are labelled `spec`) |

Before changing an area, read its code comments and open issues: they give the reasons for the design. A change that reverses a documented decision should say why in the pull request.

## Ways to help

- **Report a bug or a confusing error message.** Open a GitHub issue with the bug template. Diagnostics are a feature here, so an unhelpful compiler message counts as a bug.
- **Propose an idea.** Open a GitHub issue with the idea template before writing code, so we can check it against the plan and earlier decisions.
- **Pick up a ticket.** Work tickets are GitHub issues. An open, unassigned ticket labelled `ready-for-human` whose `Blocked by` issues are all closed is free to take; specs and issues labelled `grilling`, `needs-triage` or `needs-info` are not, because their design is still open. Comment on it first to avoid duplicate work.

Everything, from maintainer work to outside reports and ideas, is tracked in GitHub issues.

## Development

Requirements: a stable Rust toolchain at or above the `rust-version` in [Cargo.toml](Cargo.toml). Node 20 or later is needed only for the browser tests, and Elixir only to regenerate conformance fixtures.

```bash
scripts/verify.sh          # what a pull request must pass: fmt, clippy -D warnings, all Rust tests
scripts/verify.sh --full   # also the browser tests and cargo-deny
```

### The test seams

Each seam answers a different question. They are listed from the one most tests use. Every behaviour a user could observe has a test in one of them.

| Seam | What it checks | Run it |
|---|---|---|
| Unit and in-process | The application's network boundary, driven as a tower service: routing, Pipelines, controllers, session, CSRF, Dead render, join, Events, Messages, navigation, forms, errors. Also the Changeset in `griffin-domain`. | `cargo test --workspace` (no Node, no Elixir, no network) |
| Conformance with Phoenix | Griffin's diff JSON against fixtures captured from real Phoenix LiveView, for a catalogue of templates. | `cargo test -p griffin-web --test conformance`. Regenerate fixtures: `elixir crates/griffin-web/tests/conformance/generate.exs` (Elixir and Hex; the change shows up as a diff to review). Adding a case: [crates/griffin-web/tests/conformance/README.md](crates/griffin-web/tests/conformance/README.md) |
| Compile-fail (trybuild) | The text of each compiler diagnostic: `crates/griffin-web/tests/ui/` and `crates/griffin-domain/tests/ui/`, one `.rs` and its `.stderr`. | `cargo test -p griffin-web --test html syntax_and_type_errors_are_reported_at_the_template` and `cargo test -p griffin-domain --test changeset derive_and_validate_diagnostics`. After a wording change, review and accept the new text with `TRYBUILD=overwrite` before the command. rustc output differs a little between releases, so run it with the toolchain `rust-version` allows. |
| Generated project | `cargo griffin new` into a temporary directory, then build the project, run its own tests and clippy, and run `cargo griffin dev` and `routes` in it. | `cargo test -p cargo-griffin` (offline: it reuses this repository's `Cargo.lock`). One test downloads the real esbuild and Tailwind, about 100 MB, and is ignored by default: `cargo test -p cargo-griffin --test new -- --ignored` |
| Real browser | Headless Chromium with the unmodified Phoenix client against the examples in `examples/`: handshake, ordering, reconnect, forms, navigation. | `npm ci --prefix browser-tests`, `npx --prefix browser-tests playwright install chromium`, then `npm test --prefix browser-tests`. `BROWSER_TEST_PORT` moves the first port. See [browser-tests/README.md](browser-tests/README.md) |

### Upgrading the pinned client version

Griffin conforms to one exact release of the Phoenix client (Griffin speaks Phoenix's wire protocol unchanged), now phoenix 1.8.15 and phoenix_live_view 1.2.12. An upgrade is one change, in this order:

1. Change the versions in `pins` in `crates/griffin-web/tests/conformance/generate.exs` and run it. The fixture changes are the protocol changes; review them.
2. Make the Rust tests pass again, and change `CLIENT_VERSION` in `crates/griffin-web/src/live.rs`.
3. Update the versions the documents state (`README.md`, this file, `crates/cargo-griffin/templates/root/README.md`, the vendor README); `crates/griffin-web/tests/pinned_versions.rs` fails if one of those four files lacks the pinned versions, or has an `X.Y.Z` after a mention of phoenix or LiveView on the same line that is not the pin. Replace the vendored bundles in `examples/counter/assets/vendor/` and in `crates/cargo-griffin/vendor/` (the steps are in `examples/counter/assets/vendor/README.md`) and update the checksums there.
4. Run `scripts/verify.sh --full`, and say in the pull request what the protocol diff was.

## Pull requests

- Keep a pull request to one ticket or one idea.
- Add or update tests that check behaviour a user could observe: bytes on the wire, HTML in a response, a compiler message. Avoid tests that pin internal structure.
- Reuse the names the code already uses for a concept; do not coin a second word for it.
- Tick the ticket's acceptance criteria if your change completes them, and make sure the PR is in the issue's milestone.
- Write commit messages that say what changed and why.

## Licensing

Griffin is licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option. Unless you state otherwise, any contribution you intentionally submit for inclusion in the work, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
