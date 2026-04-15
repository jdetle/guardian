# Contributing to Guardian

## Branching and pull requests

- **Open a pull request** against `main` for substantive changes. Avoid pushing directly to `main`.
- Use a dedicated branch; prefer `git worktree` (see project agent rules) so the default branch stays clean.

## Versioning (SemVer)

The crate version in **`Cargo.toml`** tracks releases of **`guardiand`** and the **`guardian`** CLI.

- **PATCH** — bugfixes, documentation, tests, CI, or safe refactors with no user-visible contract change.
- **MINOR** — new features or additive behavior that remain backward compatible.
- **MAJOR** — breaking changes to CLI, daemon behavior, or configuration that users rely on.

**Bump the version in `Cargo.toml` on each PR** that is not purely trivial, so `main` always reflects a coherent next release.

## Reviews and checks

- Before push: multi-model adversarial review (see `.cursor/rules/pre-push-adversarial-review.mdc`).
- If the change touches install scripts, plists, or shipped Rust binaries: run the install smoke test (see `.cursor/rules/pr-install-smoke-test.mdc`).

## License

By contributing, you agree your contributions are licensed under the same terms as the project (MIT — see `LICENSE`).
