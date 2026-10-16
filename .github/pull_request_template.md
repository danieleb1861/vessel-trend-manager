## What and why

<!-- The user-visible problem and the final behavior. -->

## Verification

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --all-targets --locked --offline -- -D warnings`
- [ ] `cargo test --locked --offline`
- [ ] `cargo run --locked --offline -- --smoke-test --demo`
- [ ] Checked manually with **Try demo** in English and Italian (GUI changes)

## Checklist

- [ ] No real archives, values, tag lists, vessel identities, screenshots, preferences or personal paths
- [ ] New GUI texts have English and Italian versions (`t()` / `tr!`)
- [ ] Analysis or ML changes use synthetic data and fixed seeds, and label outputs as exploratory hints to verify
- [ ] New public files added to both `.gitignore` and `publicTrend` in `build.ps1`
- [ ] Dependency changes (if any) explained, with `Cargo.lock` and licenses reviewed
