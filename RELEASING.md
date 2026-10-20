# Releasing

TPM follows master, so master must never carry a version whose release doesn't
exist yet. The Release workflow takes care of that: it moves master itself, last.

1. Push any change to `.github/workflows/` to master first, on its own. The
   workflow's token is not allowed to push workflow changes, so a release whose
   commits include one publishes but cannot move master.
2. Bump `version` in `Cargo.toml` (and `Cargo.lock`). In `CHANGELOG.md`, rename
   `## [Unreleased]` to `## [X.Y.Z] - YYYY-MM-DD`, add a new, empty
   `## [Unreleased]` above it, and update the compare links at the bottom.
   Commit both in the commit you tag. Don't push master.
3. Tag it and push the tag alone: `git tag vX.Y.Z && git push origin vX.Y.Z`.
   Never move a tag once pushed; the workflow refuses a tag that no longer
   names the commit it started from. If something is wrong, cut a new version.
4. The Release workflow then, in order:
   - builds the 4 targets and runs the full test suite (`ci.yml`); nothing is
     published unless both pass;
   - publishes the release with all 8 assets (4 tarballs, 4 `.sha256`).
     Published releases are never overwritten;
   - `tap`: updates the Homebrew tap (plopezlpz/homebrew-tap) with the
     `HOMEBREW_TAP_DEPLOY_KEY` secret, a write deploy key for the tap only. It
     never replaces a newer version, and fails if the secret is missing;
   - `master`: fast-forwards master to the tagged commit (TPM follows master)
     and redeploys the site.
5. If a step failed, finish it by hand:
   - tap: `scripts/update-tap.sh X.Y.Z` (it uses your own SSH credentials), or
     commit the output of `scripts/update-formula.sh X.Y.Z` to the tap as `Formula/paddock.rb`;
   - master: `git push origin 'vX.Y.Z^{commit}:master' && gh workflow run pages.yml --ref master`
     (only once the release is published).
6. The workflow publishes GitHub's generated notes. Once the release is published,
   replace them with the version's `CHANGELOG.md` section (its body, without the heading):
   `awk '/^## /{p=0} /^## \[X\.Y\.Z\]/{p=1; next} p' CHANGELOG.md | gh release edit vX.Y.Z --notes-file -`

A pre-release tag (`vX.Y.Z-rc1`) is published as a pre-release, never becomes
"Latest", and leaves the tap and master alone.

If the UI changed, re-record the demos before the release: `cd site/tools && npm run record && node build-svg.mjs`,
then commit `site/assets/frames` and `docs/brand`. (Re-record sparingly: every run adds blobs to every TPM clone.)

Any change to the popup's flags or env is a version bump, released (steps 2-4)
before master carries it.
