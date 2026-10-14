# Contributing

Keep changes small and explain the user-visible problem, the final behavior and
the verification performed. Four application modules are intentional; introduce
another module only when it removes a clear responsibility from an existing one.

## Before sharing

- Use synthetic fixtures. Do not submit `.s3db` files, real operating values,
  screenshots, schedules, vessel identities, sensor configurations, thresholds,
  personal paths, credentials, local preferences or build outputs.
- Report an incompatible schema using an anonymized table/column description
  and a minimal synthetic database created by a test.
- Review the diff and the files actually staged for commit. Ignore rules do not
  remove data that is already tracked or present in Git history.
- Contributions are accepted under the project's [MIT License](LICENSE).

## Changes and tests

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets --locked --offline -- -D warnings
cargo test --locked --offline
cargo run --locked --offline -- --smoke-test
```

For changes to retrieval, exercise interval boundaries, quality flags, missing
archives, cancellation and bounded reduction. Preserve source units and timestamps;
keep daily observations distinct from aggregates and old state records.

For another archive format, preserve the `scan`/`load` boundary in `src/archive.rs`, described in
[MAINTAINING.md](MAINTAINING.md). Avoid duplicating the GUI or embedding machinery
names from a particular vessel. Query source archives in read-only mode.

Keep `Cargo.lock` in version control. Review dependency changes and license files.
Do not include local `.rust`, `.private`, `target` or `dist` folders.
Contributions made with coding agents follow [AGENTS.md](AGENTS.md).

Update the operator guide only when the operator's workflow changes. Describe
build or adaptation steps in `MAINTAINING.md`; keep the repository README short.
