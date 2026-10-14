# Releasing

TPM follows master, so master must never carry a version whose release doesn't
exist yet. The Release workflow takes care of that: it moves master itself, last.

1. Bump `version` in `Cargo.toml` (and `Cargo.lock`) and commit. Don't push master.
2. Tag it and push the tag alone: `git tag vX.Y.Z && git push origin vX.Y.Z`.
3. The Release workflow then, in order:
   - builds the 4 targets and publishes the release with all 8 assets
     (4 tarballs, 4 `.sha256`). Published releases are never overwritten; if
     something is wrong, cut a new version;
   - `tap`: updates the Homebrew tap (plopezlpz/homebrew-tap) with the
     `HOMEBREW_TAP_DEPLOY_KEY` secret, a write deploy key for the tap only.
     Pre-release tags (`vX.Y.Z-rc1`) leave the tap alone;
   - `master`: fast-forwards master to the tagged commit (TPM
     follows master) and redeploys the site.
4. If a step failed, finish it by hand:
   - tap: `HOMEBREW_TAP_DEPLOY_KEY=... scripts/update-tap.sh X.Y.Z`, or commit
     the output of `scripts/update-formula.sh X.Y.Z` to the tap as `Formula/paddock.rb`;
   - master: `git push origin vX.Y.Z^{commit}:master` (only after the release is published).

If the UI changed, re-record the demos before the release: `cd site/tools && npm run record && node build-svg.mjs`,
then commit `site/assets/frames` and `docs/brand`. (Re-record sparingly: every run adds blobs to every TPM clone.)

Any change to the popup's flags or env is a version bump, released (steps 2-3)
before master carries it.
