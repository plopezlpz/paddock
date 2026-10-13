#!/usr/bin/env bash
# Prints the Homebrew formula for a paddock release on stdout, with the sha256s
# of its four tarballs: commit the output to the tap as Formula/paddock.rb.
# Usage: scripts/update-formula.sh [--from DIR] 0.1.0|v0.1.0 > paddock.rb
# The hashes come from the release's .sha256 assets (gh), or from DIR holding
# copies of them (offline, for tests).
set -euo pipefail

usage() { echo 'usage: scripts/update-formula.sh [--from DIR] VERSION' >&2; exit 2; }
from=
if [ "${1:-}" = --from ]; then
  [ $# -ge 2 ] || usage
  from=$2
  shift 2
fi
[ $# -eq 1 ] || usage
v=${1#v}
# Strict, because it goes into sed and a URL.
printf '%s\n' "$v" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$' ||
  { echo "paddock: '$1' is not a version like 0.1.0" >&2; exit 2; }

if [ -z "$from" ]; then
  command -v gh >/dev/null || { echo 'paddock: needs gh (or --from DIR)' >&2; exit 1; }
  from=$(mktemp -d)
  trap 'rm -rf "$from"' EXIT
  gh release download "v$v" -R plopezlpz/paddock -p 'paddock-*.tar.gz.sha256' -D "$from" >&2
fi

sha() { # sha TARGET: the validated hash from TARGET's .sha256, which must name its tarball.
  local asset=paddock-$1.tar.gz want name
  [ -r "$from/$asset.sha256" ] || { echo "paddock: cannot read $from/$asset.sha256" >&2; exit 1; }
  read -r want name <"$from/$asset.sha256" || true
  printf '%s\n' "$want" | grep -Eq '^[0-9a-f]{64}$' ||
    { echo "paddock: $asset.sha256 does not hold a sha256" >&2; exit 1; }
  [ "${name#\*}" = "$asset" ] ||
    { echo "paddock: $asset.sha256 names '$name', not $asset" >&2; exit 1; }
  printf '%s' "$want"
}
a_darwin=$(sha aarch64-apple-darwin)
x_darwin=$(sha x86_64-apple-darwin)
a_linux=$(sha aarch64-unknown-linux-musl)
x_linux=$(sha x86_64-unknown-linux-musl)

# Quoted, so the Ruby's #{...} and quotes stay as written; sed fills the rest.
sed -e "s/@VERSION@/$v/g" \
  -e "s/@SHA_AARCH64_DARWIN@/$a_darwin/" \
  -e "s/@SHA_X86_64_DARWIN@/$x_darwin/" \
  -e "s/@SHA_AARCH64_LINUX@/$a_linux/" \
  -e "s/@SHA_X86_64_LINUX@/$x_linux/" <<'EOF'
class Paddock < Formula
  desc "Fast tmux popup for sessions, windows, panes and the AI agents in them"
  homepage "https://github.com/plopezlpz/paddock"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/plopezlpz/paddock/releases/download/v@VERSION@/paddock-aarch64-apple-darwin.tar.gz"
      sha256 "@SHA_AARCH64_DARWIN@"
    end
    on_intel do
      url "https://github.com/plopezlpz/paddock/releases/download/v@VERSION@/paddock-x86_64-apple-darwin.tar.gz"
      sha256 "@SHA_X86_64_DARWIN@"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/plopezlpz/paddock/releases/download/v@VERSION@/paddock-aarch64-unknown-linux-musl.tar.gz"
      sha256 "@SHA_AARCH64_LINUX@"
    end
    on_intel do
      url "https://github.com/plopezlpz/paddock/releases/download/v@VERSION@/paddock-x86_64-unknown-linux-musl.tar.gz"
      sha256 "@SHA_X86_64_LINUX@"
    end
  end

  def install
    bin.install "paddock"
  end

  def caveats
    <<~EOS
      paddock needs tmux 3.8 or later (tmux -V). Add to ~/.tmux.conf:

        set -g @paddock-key 'C'
        run-shell '#{HOMEBREW_PREFIX}/bin/paddock init'

      Reload tmux after installing and after every upgrade.
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/paddock --version")
  end
end
EOF
