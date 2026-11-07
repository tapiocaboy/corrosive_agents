# Publishing `corrosive_agents` to crates.io

A step-by-step release checklist. The crate name `corrosive_agents` was free
as of 2026-07-05 — re-check before the first publish (step 2).

There are two ways to publish:

- **Automated (recommended):** publish a GitHub Release and let the
  [Release workflow](.github/workflows/release.yml) test + publish to
  crates.io — see [§ Automated publishing](#automated-publishing-github-actions).
- **Manual:** run the steps below from your machine.

CI ([ci.yml](.github/workflows/ci.yml)) runs on every pull request and every
push to `main`, but **never publishes** — its `cargo publish --dry-run` job is
a packaging check only. Publishing happens exclusively when a GitHub Release
is published.

Either way, do the one-time setup and pre-flight checks first.

## 0. One-time setup

1. Create an account at <https://crates.io> (sign in with GitHub).
2. Verify your email address at <https://crates.io/settings/profile>
   (publishing is blocked without it).
3. Create an API token at <https://crates.io/settings/tokens>
   (scope: `publish-new` + `publish-update`).
4. Log in locally — the token is stored in `~/.cargo/credentials.toml`:

   ```sh
   cargo login <your-token>
   ```

## 1. Pre-flight checks

```sh
# Everything must be green, across feature combinations:
cargo fmt --check
cargo clippy --all-targets --features full -- -D warnings
cargo test --features full
cargo build --no-default-features            # feature gates don't leak
cargo build --no-default-features --features grpc
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --features full
cargo +1.88 check --all-targets --features full   # MSRV (matches CI)
cargo deny check                    # supply-chain: advisories, licenses, bans, sources
```

> **Lockfile freshness:** `cargo deny check` fails (`yanked = "deny"`) when the
> pinned `Cargo.lock` has drifted into a yanked crate or a new RustSec
> advisory — common after a few months, even with no code change. Fix it with a
> **targeted** update of just the flagged crates, e.g.
> `cargo update -p tokio -p rustls -p h2`. Avoid a blanket `cargo update`: it
> can pull a dependency that raises the MSRV above 1.88 (e.g. a newer `uuid`),
> which breaks the MSRV job. Re-run the MSRV check after any lockfile bump.

Review `Cargo.toml` metadata — crates.io renders all of it:

- [ ] `version` bumped per [SemVer](https://semver.org) (pre-1.0: breaking
      changes bump the **minor** version)
- [ ] `description`, `keywords` (max 5), `categories` (must be from
      [the official list](https://crates.io/category_slugs))
- [ ] `repository` points at the real GitHub repo (create/push it first)
- [ ] `license = "MIT OR Apache-2.0"` and both `LICENSE-*` files exist
- [ ] `readme = "README.md"`
- [ ] `exclude` keeps secrets and dev-only files out of the package
      (**`.env` must never ship** — verify in step 2)

## 2. Verify the package contents

```sh
# Name still free? (404 = free)
curl -s https://crates.io/api/v1/crates/corrosive_agents | head -c 120

# Exactly what would be uploaded:
cargo package --list --features full

# Dry-run: builds the crate from the packaged sources in isolation
cargo publish --dry-run --features full
```

Check the `--list` output: no `.env`, no `target/`, no `tools/`, no
`usecases/` (the libp2p use case has its own workspace and must stay out of the
package). The vendored `src/grpc/pb.rs` **must** be present (users don't run
`protoc`).

## Automated publishing (GitHub Actions)

One-time setup, after creating the GitHub repository:

1. Create a crates.io API token with publish scope
   (<https://crates.io/settings/tokens>).
2. Add it as a repository secret named `CARGO_REGISTRY_TOKEN`:
   GitHub → *Settings → Secrets and variables → Actions → New repository secret*
   (or: `gh secret set CARGO_REGISTRY_TOKEN`).

Then every release is just:

```sh
# bumps Cargo.toml, stamps CHANGELOG.md, commits, tags vX.Y.Z, pushes:
cargo release patch --execute     # or: minor / major / an explicit version
# publish the release — THIS triggers the crates.io upload:
gh release create v0.0.3 --generate-notes
```

(`cargo release` comes from [`cargo-release`](https://github.com/crate-ci/cargo-release):
`cargo install cargo-release`. Its behavior is configured under
`[package.metadata.release]` in Cargo.toml — it deliberately does **not**
publish to crates.io itself; CI does that on the GitHub Release event.
Without the tool, bump the version and stamp CHANGELOG.md by hand:)

```sh
# manual equivalent: edit Cargo.toml version + CHANGELOG.md, then
git commit -am "Release v0.0.3" && git push
gh release create v0.0.3 --generate-notes
```

(or draft and publish the release in the GitHub UI — the workflow fires on
**release published**, not on tag pushes or merges).

The [Release workflow](.github/workflows/release.yml) will:

1. check the release tag matches the `Cargo.toml` version (mismatch fails the run),
2. run fmt + clippy + the full test suite,
3. `cargo publish --locked` to crates.io.

The [CI workflow](.github/workflows/ci.yml) runs on every pull request and
every push to `main`: rustfmt, clippy (`-D warnings`), the test matrix across
feature combinations (default / none / `grpc` / `full`), rustdoc
(`-D warnings`), an MSRV check on Rust 1.88, cargo-deny, coverage, and a
`cargo publish --dry-run` packaging check — none of which upload anything.

## 3. Tag and publish (manual alternative)

```sh
git add -A && git commit -m "Release v0.1.0"
git tag -a v0.1.0 -m "corrosive_agents v0.1.0"
git push origin main --tags

cargo publish
```

Publishing is **permanent** — versions can be yanked but never deleted or
re-uploaded, so the dry-run matters.

## 4. Post-publish

1. Check the crate page: <https://crates.io/crates/corrosive_agents>.
2. docs.rs builds automatically within ~15 minutes:
   <https://docs.rs/corrosive_agents>. The `[package.metadata.docs.rs]`
   section already enables `all-features` so `grpc`/`qdrant`/`pinecone` APIs
   are documented.
3. If a bad version slipped out: `cargo yank --version 0.1.0`
   (and `--undo` to reverse).
4. Add co-owners if wanted: `cargo owner --add <github-user>`.

## Releasing subsequent versions

1. Update `version` in `Cargo.toml`.
2. Update `CHANGELOG.md`: move the `[Unreleased]` entries under a new
   `## [X.Y.Z] - <date>` heading (`cargo release` does this automatically via
   the `pre-release-replacements` in `Cargo.toml`).
3. If `proto/agent.proto` changed, regenerate the vendored gRPC code and
   commit it:

   ```sh
   cargo run --manifest-path tools/protogen/Cargo.toml
   ```

4. Repeat steps 1–4 above.

Consider [`cargo-release`](https://github.com/crate-ci/cargo-release) to
automate the bump/tag/publish sequence, and
[`cargo-semver-checks`](https://github.com/obi1kenobi/cargo-semver-checks)
to catch accidental API breakage before it ships.
