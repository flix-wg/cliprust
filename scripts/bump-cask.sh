#!/bin/bash
# Points Casks/cliprust.rb at a published release: bump-cask.sh 0.2.0
set -euo pipefail

VERSION="${1:?usage: $0 VERSION}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SUMS="https://github.com/flix-wg/cliprust/releases/download/v$VERSION/SHA256SUMS.txt"
SHA="$(curl -fsSL "$SUMS" | awk -v f="ClipRust-$VERSION-macos-arm64.zip" '$2 == f { print $1 }')"
[ -n "$SHA" ] || { echo "No checksum for ClipRust-$VERSION-macos-arm64.zip in $SUMS" >&2; exit 1; }

sed -i.bak -e "s/^  version \".*\"/  version \"$VERSION\"/" -e "s/^  sha256 \".*\"/  sha256 \"$SHA\"/" \
    "$ROOT/Casks/cliprust.rb"
rm "$ROOT/Casks/cliprust.rb.bak"
echo "Casks/cliprust.rb → $VERSION ($SHA)"
