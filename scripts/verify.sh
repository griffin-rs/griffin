#!/usr/bin/env bash
# The verify gate. It must pass before a change is ready for review.
#   scripts/verify.sh         fast gate: fmt, clippy, tests
#   scripts/verify.sh --full  also browser tests and supply-chain checks
set -euo pipefail
cd "$(dirname "$0")/.."

step() { printf '\n== %s\n' "$*"; }

step "cargo fmt --check"
cargo fmt --all --check

step "cargo clippy (warnings are errors)"
cargo clippy --workspace --all-targets --quiet -- -D warnings

step "cargo test"
cargo test --workspace --quiet

if [[ "${1:-}" == "--full" ]]; then
  step "browser tests (Playwright)"
  (cd browser-tests && npm test)

  step "cargo deny"
  cargo deny --version >/dev/null 2>&1 || { echo "cargo-deny not installed: cargo install cargo-deny --locked" >&2; exit 1; }
  cargo deny check
fi

step "verify passed"
