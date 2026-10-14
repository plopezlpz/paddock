# Releasing

TPM follows master, so master must never carry a version whose release doesn't exist yet.

1. Bump `version` in `Cargo.toml` (and `Cargo.lock`) and commit.
2. Tag it and push the tag alone: `git tag vX.Y.Z && git push origin vX.Y.Z`.
3. Wait for the Release workflow to publish the release with all 8 assets
   (4 tarballs, 4 `.sha256`). Published releases are never overwritten; if
   something is wrong, cut a new version.
4. Only then push master: `git push origin master`.
5. The Release workflow's `tap` job updates the Homebrew tap
   (plopezlpz/homebrew-tap) right after publishing, using the
   `HOMEBREW_TAP_DEPLOY_KEY` secret (a write deploy key for the tap only). If
   it was skipped or failed, run it by hand: `scripts/update-tap.sh X.Y.Z`
   with `HOMEBREW_TAP_DEPLOY_KEY` set, or commit the output of
   `scripts/update-formula.sh X.Y.Z` to the tap as `Formula/paddock.rb`.
   Pre-release tags (`vX.Y.Z-rc1`) do not touch the tap.

TPM needs nothing: it follows master, so step 4 is its release. `cargo install
--git` also follows master.

If the UI changed, re-record the demos before the release: `cd site/tools && npm run record && node build-svg.mjs`,
then commit `site/assets/frames` and `docs/brand`. (Re-record sparingly: every run adds blobs to every TPM clone.)

Any change to the popup's flags or env is a version bump, released (steps 2-4)
before master carries it.
