# Releasing

TPM follows master, so master must never carry a version whose release doesn't exist yet.

1. Bump `version` in `Cargo.toml` (and `Cargo.lock`) and commit.
2. Tag it and push the tag alone: `git tag vX.Y.Z && git push origin vX.Y.Z`.
3. Wait for the Release workflow to publish the release with all 8 assets
   (4 tarballs, 4 `.sha256`). Published releases are never overwritten; if
   something is wrong, cut a new version.
4. Only then push master: `git push origin master`.
5. Update the Homebrew tap: `scripts/update-formula.sh X.Y.Z` (prints the
   formula; the hashes come from the release), then commit it to the tap.

Any change to the popup's flags or env is a version bump and a release in the
same push.
