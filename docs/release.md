# Release train — Sunbeam Compute Platform CLI

How a `sunbeam` release goes from merged code to installed binary.
Everything runs on GitHub Actions (the old WFE/Gitea pipeline is gone).

| Piece | File | Job |
|---|---|---|
| CI | `.github/workflows/ci.yml` | fmt, clippy, nextest; **cut the version tag** on mainline |
| Release builds | `.github/workflows/release.yml` | build binaries, publish the **GitHub release** |
| Homebrew tap | `sunbeamdotpt/tap` repo | deliver to users (`brew install sunbeam`) |

## The train, step by step

1. **Develop on branches.** All gates run locally before merge:
   `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`,
   `cargo nextest run`. CI runs the same on every PR.

2. **Bump the version when you're ready to ship.** `Cargo.toml` `version`
   is the single source of truth. Semver: breaking CLI changes → major,
   new verbs/features → minor, fixes → patch. The bump is a normal commit
   on the release branch before merge.

3. **Merge to `mainline`.** After lint + tests pass, the `tag` job in
   `ci.yml` reads the version from `Cargo.toml`; if tag `vX.Y.Z` doesn't
   exist it creates and pushes it, then dispatches the release workflow
   (GITHUB_TOKEN-pushed tags don't cascade to `on: push` workflows, hence
   the explicit dispatch). No version bump → tag exists → nothing happens.

4. **`release.yml` builds and publishes.** Builds on four targets
   (`aarch64`/`x86_64` × macOS/Linux; x86_64-apple is cross-compiled on the
   arm runner and packaged via Rosetta 2 — Intel runners are
   capacity-starved), each with:
   - the platform's public SSO client ID baked in at compile time via the
     `SUNBEAM_SSO_CLIENT_ID` repo secret (option_env! — never in git),
   - `buf` for the sdk's ConnectRPC codegen,
   - man pages (`sunbeam __man`, gzipped) and shell completions,
   packaged as `sunbeam_X.Y.Z_<target>.tar.gz` **plus** raw
   `sunbeam-raw-<target>` binaries (what `sunbeam update` downloads), then
   creates the GitHub release with everything plus `checksums.txt` and
   auto-generated notes.

5. **Update the Homebrew tap.** The formula installs the prebuilt release
   tarballs (no build toolchain for users). The release workflow prints the
   four tarball sha256s into its job summary. In the tap repo
   (`../tap`, github.com/sunbeamdotpt/tap):
   - bump the four `url`s to the new version and set the four `sha256`s,
   - `brew audit sunbeamdotpt/tap/sunbeam`,
     `brew install sunbeamdotpt/tap/sunbeam`,
     `brew test sunbeamdotpt/tap/sunbeam`,
   - commit and push. Users get the release via `brew upgrade sunbeam`.

6. **Verify.** The GH release page shows all four tarballs + four raw
   binaries + checksums; `sunbeam update` on the previous release moves to
   the new version; `brew info sunbeam` resolves it after the tap push.

## Rebuilding and fixing

- **Rebuild artifacts for an existing tag**: Actions → release →
  Run workflow → enter the tag. Same tag, fresh artifacts — never move a
  tag.
- **Broken release**: do not re-tag or edit published artifacts. Cut a
  patch release (bump patch, back to step 2) and optionally mark the
  broken GH release as a pre-release.
- **Hotfix**: branch from the tag, cherry-pick, bump patch, merge — the
  train is identical.

## Secrets and prerequisites

- `SUNBEAM_SSO_CLIENT_ID` (GitHub repo secret) — the provisioned public
  OAuth2 client ID, baked into release binaries. If unset, binaries build
  fine but `sunbeam auth login` requires the runtime env var.
- Builds need network access (crates.io + buf.build for the sdk codegen).
- The tap formula installs the pre-baked release binaries, so the client
  ID reaches brew users automatically; `SUNBEAM_SSO_CLIENT_ID` at runtime
  remains the override for non-release builds.

## Self-update channel

`sunbeam update` tracks this train: it queries the GitHub API for the
latest release, compares the tag to its own `CARGO_PKG_VERSION`, downloads
the `sunbeam-raw-<target>` asset for its platform, verifies it against
`checksums.txt`, and atomically replaces itself. (The old Gitea
CI-artifact channel was removed with the WFE pipeline.)

## Version numbering notes

- `2.0.0-rc*` pre-releases existed historically; current train starts at
  `3.0.0`. Release candidates, if needed again, use `X.Y.Z-rc.N` tags —
  the workflow handles any `v*` tag; mark RC releases as pre-release in
  the GH release UI after the workflow creates them.
