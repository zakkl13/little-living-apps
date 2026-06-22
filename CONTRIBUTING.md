# Contributing

Thanks for your interest in Little Living Apps. It's an open pattern, not a service — forks,
fixes, and ideas are all welcome.

## Ground rules

- **Open an issue first** for anything non-trivial, so we can agree on the shape before you
  write code. Small fixes (typos, docs, obvious bugs) can go straight to a PR.
- **One concern per PR.** Keep changes focused and reviewable.
- **The orchestrator core must stay honest.** It's the part that can't lose a message,
  double-reply, or corrupt memory: a single serialized loop, no global mutable state, no
  `unsafe`, every external boundary an injectable seam. New boundaries follow the same
  pattern so the deterministic suite can drive the real binary against fakes.

## Before you push

CI runs all of these and will reject a PR that fails any. Run them locally first:

```bash
cargo fmt --check                            # formatting (use `cargo +nightly fmt` to apply)
cargo clippy --all-targets -- -D warnings    # lints — warnings are errors
cargo test --all-targets                     # deterministic suite
scripts/coverage.sh check 78                 # core coverage gate (≥ 78%)
scripts/check-complexity.sh                  # cyclomatic complexity ≤ 6 per function
```

Notes:
- Targets **Rust edition 2024**. Keep functions small — clippy caps cognitive complexity and
  argument count at 6, and bodies at 60 lines.
- **Add tests with behavior changes.** Boundaries are faked, so most logic is testable
  without a real model. Don't claim a fix works without a test or a manual check that proves it.
- The live model/eval paths (`#[ignore]`d `live_*` tests, `lila-eval`) spend real subscription
  tokens and run against a real account — you don't need them to pass CI.

## Commit & PR

- Clear, present-tense commit messages explaining *why*, not just *what*.
- In the PR description, say what you changed, how you verified it, and link the issue.

## License

By contributing, you agree your work is licensed under the [MIT License](LICENSE).
