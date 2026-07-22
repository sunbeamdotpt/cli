# Release train — Sunbeam Compute Platform CLI

How a `sunbeam` release goes from merged code to installed binary. Two CI
systems are involved, on purpose:

| System | File | Job |
|---|---|---|
| WFE pipeline | `workflows.yaml` | lint, tests, **cut the version tag** on mainline |
| GitHub Actions | `.github/workflows/release.yml` | build binaries, publish the **GitHub release** |
| Homebrew tap | `sunbeamdotpt/tap` repo | deliver to users (`brew install sunbeam`) |

## The train, step by step

1. **Develop on branches.** All gates run locally before merge:
   `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`,
   `cargo nextest run`. Merges land on `mainline`.

2. **Bump the version when you're ready to ship.** `Cargo.toml` `version`
   is the single source of truth. Semver: breaking CLI changes → major,
   new verbs/features → minor, fixes → patch. The bump is a normal commit
   on the release branch before merge.

3. **WFE CI runs on the merge.** Lint + full test suite (including
   integration suites; they skip without docker). On `mainline` only, the
   pipeline reads the version from `Cargo.toml` and pushes tag
   `vX.Y.Z` to GitHub. If the tag already exists it does not re-tag —
   forget the version bump and the release simply doesn't happen.

4. **The tag triggers `.github/workflows/release.yml`.** It builds
   `--release --locked` binaries natively on four targets
   (`aarch64`/`x86_64` × macOS/Linux), each with:
   - the platform's public SSO client ID baked in at compile time via the
     `SUNBEAM_SSO_CLIENT_ID` repo secret (option_env! — never in git),
   - `buf` for the sdk's ConnectRPC codegen,
   - man pages (`sunbeam __man`, gzipped) and shell completions,
   packaged as `sunbeam_X.Y.Z_<target>.tar.gz`, then creates the GitHub
   release with all tarballs, a `checksums.txt`, and auto-generated notes.

5. **Update the Homebrew tap.** The release workflow prints the source
   archive's sha256 into its job summary. In the tap repo
   (`../tap`, github.com/sunbeamdotpt/tap):
   - set `Formula/sunbeam.rb` `sha256` to that value (and `url` version on
     future releases),
   - `brew audit --new Formula/sunbeam.rb`,
     `brew install --build-from-source Formula/sunbeam.rb`,
     `brew test Formula/sunbeam.rb`,
   - commit and push. Users get the release via `brew upgrade sunbeam`.

6. **Announce / close out.** Verify the GH release page shows all four
   artifacts + checksums, and `brew info sunbeam` resolves the new version
   after the tap push.

## Rebuilding and fixing

- **Rebuild artifacts for an existing tag**: Actions → release →
  Run workflow → enter the tag. Same artifacts, same tag — never move a
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
- WFE CI prerequisites are documented in `workflows.yaml` (wfe-server,
  credentials secret, CI image); compiling steps install pinned buf.

## Update channels (note)

`sunbeam update` (self-update) currently pulls the latest **mainline CI
artifact** from the Gitea Actions API (`src.{domain}/api/v1/repos/studio/cli`)
— a per-commit bleeding-edge channel, independent of this release train.
Installed v2 binaries depend on that Gitea artifact shape, so don't change
it casually (see `.maintainer/known-issues.md`). Whether v3 self-update
should switch to tagged GH releases is an open human decision; the GH
release artifacts published by this train are the natural target.

## Version numbering notes

- `2.0.0-rc*` pre-releases existed historically; current train starts at
  `3.0.0`. Release candidates, if needed again, use `X.Y.Z-rc.N` tags —
  the workflow handles any `v*` tag; mark RC releases as pre-release in
  the GH release UI after the workflow creates them.
