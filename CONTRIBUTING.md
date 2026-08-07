# Contributing to Drift

Thanks for helping improve Drift. Focused bug fixes, tests, documentation, and small UI
improvements are the easiest contributions to review and ship.

## Before you start

- Search the existing issues before opening a new one.
- Use the bug report form for reproducible defects.
- Open a feature request before investing in a substantial behavior or architecture
  change. This avoids work on a direction that may not fit the project.
- Report security vulnerabilities privately according to [SECURITY.md](SECURITY.md).
- Follow the [Code of Conduct](CODE_OF_CONDUCT.md) in every project space.

## Development setup

Drift's native target is Windows x64. You need [Bun](https://bun.sh), a
[stable Rust toolchain](https://rustup.rs/) with the MSVC target,
[Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/)
with the **Desktop development with C++** workload, and the
[Microsoft Edge WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/).

```bash
git clone https://github.com/kylepelham/Drift.git
cd Drift
bun install
bun install --ignore-scripts --cwd engine/upstream
bun install --cwd engine/opencode
bun run build:engine
```

Start the engine and Vite UI with:

```bash
bun run dev
```

For the native development window, leave `bun run dev` running and execute
`bunx tauri dev` in a second terminal.

## Architecture boundaries

Read [docs/architecture.md](docs/architecture.md) before making structural changes. The
important boundaries are:

- UI components read the engine store and call engine actions; they do not fetch engine
  endpoints directly.
- `src/engine/` owns engine transport, hydration, events, and engine-derived state.
- `src/state/` owns Drift application state and native-store facades.
- Drift-specific persistence belongs in the Tauri-owned SQLite store, not OpenCode's
  storage.
- `engine/upstream/` is a pristine OpenCode snapshot. Never edit it directly.
- Internal OpenCode adaptations belong in small patches under `engine/overlays/` only
  when a public engine API or plugin cannot express the behavior.

Prefer the smallest correct change. Match established UI patterns, keep unrelated
cleanup out of the pull request, and add comments only where the reason is not evident
from the code.

## Testing

Run the checks relevant to your change before opening a pull request. The standard suite
is:

```bash
bun run typecheck
bun run test
cargo test --manifest-path src-tauri/Cargo.toml
```

Also run:

- `bun run test:engine` for engine overlays or Drift-shipped OpenCode extensions.
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` for Rust changes.
- `bun run build:native` when changing packaging, native integration, generated
  extensions, or release behavior.

Tests should cover observable behavior and regressions rather than implementation
details. Keep fixtures free of real API keys, tokens, personal data, and proprietary
source.

## Updating OpenCode

The scheduled `OpenCode update` workflow opens one review pull request when upstream
`dev` advances and can also be run manually. It uses a dedicated remote ref, records the
imported SHA in `engine/upstream.commit`, dispatches CI for the generated branch, and never
merges automatically. The marker keeps later updates reliable regardless of the PR merge
strategy.

A local update uses:

```bash
git fetch --no-tags https://github.com/sst/opencode.git +dev:refs/remotes/opencode-update/dev
current="$(tr -d '\r\n' < engine/upstream.commit)"
latest="$(git rev-parse refs/remotes/opencode-update/dev)"
git rm -r --quiet engine/upstream
git read-tree --prefix=engine/upstream/ -u "refs/remotes/opencode-update/dev^{tree}"
printf '%s\n' "$latest" > engine/upstream.commit
git add engine/upstream.commit
git commit -m "chore: update vendored OpenCode to ${latest:0:10}"
git update-ref -d refs/remotes/opencode-update/dev
bun install --ignore-scripts --cwd engine/upstream
bun run test:engine
bun run build:engine
```

Do not merge, subtree-merge, or retain the temporary upstream ref: doing so makes OpenCode's
history reachable from Drift's commit graph. OpenCode also has many `v*` release tags, so
always fetch with `--no-tags`. If an overlay no longer applies, refresh that isolated patch
against the updated source instead of resolving the change inside `engine/upstream/`. The
full runbook is in [docs/engine.md](docs/engine.md).

## Releases

Releases are maintainer-only. A release tag must match `vMAJOR.MINOR.PATCH` exactly, point
to a commit contained in `origin/master`, and be strictly newer than every other stable
GitHub release or repository tag. Prerelease and build suffixes are not supported. Before
building release artifacts, the workflow checks the tag policy and runs frozen installs, typechecking, root
and engine tests, the engine build, Rust tests, and an unsigned production package build
on the exact tag commit. The release build job has read-only repository access, and only the
separate publication job has contents write access.

Stable releases are serialized across all tags. Published notes and assets are immutable:
a rerun verifies the original workflow marker and asset SHA-256 manifest, then exits
without rebuilding or publishing. Any mismatch requires investigation rather than an
overwrite.

The repository must provide these GitHub Actions secrets:

- `TAURI_SIGNING_PRIVATE_KEY`: the path or complete content of the updater private key.
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`: the private key password.

The private key must match the public key in `src-tauri/tauri.conf.json`. It is required
for every future update and must never be committed or included in workflow logs. Do not
rotate the updater key without a migration plan for already-installed copies.

After the tagged commit is merged to `master` and the standard checks pass, choose a
version newer than the latest stable release/tag and push the release tag:

```bash
git tag v1.2.3
git push origin v1.2.3
```

## Pull requests

- Keep each pull request focused on one problem.
- Explain the user-visible behavior and why the chosen change is appropriate.
- Include validation commands and their results.
- Include before/after images for visible UI changes when practical.
- Update documentation when behavior, configuration, or architecture changes.
- Do not commit build output, local databases, credentials, or generated sidecar
  binaries.

Use concise, imperative commit subjects. Maintainers may squash commits when merging.

## License

By contributing, you agree that your contribution is licensed under the project's
[MIT License](LICENSE).
