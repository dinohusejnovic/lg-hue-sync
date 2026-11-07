#!/usr/bin/env bash
# Publish a tagged release for installation through the Homebrew Channel:
#   - a GitHub release with the daemon, SHA256SUMS, the Homebrew Channel .ipk
#     and its manifest.json
#   - the repository index (api/apps.json) and icon on GitHub Pages (gh-pages)
#
# Usage: scripts/publish_hbc.sh vX.Y.Z
# Run on a clean checkout of the tagged commit after `make build-docker` and
# the device validation in docs/releasing.md. The repository is the one `gh`
# resolves for this checkout.
set -euo pipefail

TAG=${1:?Usage: scripts/publish_hbc.sh vX.Y.Z}
BINARY=target/armv7-unknown-linux-gnueabi/release/lg-hue-sync
die() { echo "publish_hbc: $*" >&2; exit 1; }

cd "$(dirname "$0")/.."
REPO=$(gh repo view --json nameWithOwner --jq .nameWithOwner)
OWNER=${REPO%%/*}
NAME=${REPO#*/}
RELEASE_URL="https://github.com/$REPO/releases/download/$TAG"
PAGES_URL="https://$(printf %s "$OWNER" | tr "[:upper:]" "[:lower:]").github.io/$NAME"

if command -v uv >/dev/null 2>&1; then PY=(uv run python); else PY=(python3); fi
"${PY[@]}" scripts/check_release.py --tag "$TAG"
git diff --quiet HEAD || die "the working tree has uncommitted changes"
[ "$(git rev-parse HEAD)" = "$(git rev-parse "$TAG^{commit}")" ] || die "HEAD is not $TAG"
git ls-remote --exit-code --tags origin "refs/tags/$TAG" >/dev/null || die "$TAG is not pushed"
[ -f "$BINARY" ] || die "build the daemon first (make build-docker)"

python3 scripts/build_hbc_package.py --binary "$BINARY" \
    --release-url "$RELEASE_URL" --repo-url "$PAGES_URL" --source-url "https://github.com/$REPO"
OUT=dist/hbc
cp "$BINARY" "$OUT/lg-hue-sync-armv7"
(cd "$OUT" && sha256sum lg-hue-sync-armv7 ./*.ipk > SHA256SUMS 2>/dev/null \
    || shasum -a 256 lg-hue-sync-armv7 ./*.ipk > SHA256SUMS)

awk -v version="${TAG#v}" '
    index($0, "## [" version "]") == 1 { found=1; next }
    found && /^## / { exit }
    found { print }
' CHANGELOG.md > "$OUT/release-notes.md"
[ -s "$OUT/release-notes.md" ] || die "CHANGELOG.md has no notes for $TAG"

ASSETS=("$OUT/lg-hue-sync-armv7" "$OUT/SHA256SUMS" "$OUT"/*.ipk "$OUT/manifest.json")
if gh release view "$TAG" >/dev/null 2>&1; then
    gh release upload "$TAG" "${ASSETS[@]}" --clobber
else
    gh release create "$TAG" "${ASSETS[@]}" --verify-tag \
        --title "LG Hue Sync $TAG" --notes-file "$OUT/release-notes.md"
fi

# Repository index on the gh-pages branch, built in a temporary worktree.
PAGES=$(mktemp -d)
trap 'git worktree remove --force "$PAGES" >/dev/null 2>&1 || true' EXIT
if git ls-remote --exit-code --heads origin gh-pages >/dev/null; then
    git fetch -q origin gh-pages
    git worktree add -q "$PAGES" FETCH_HEAD
    git -C "$PAGES" checkout -q -B gh-pages
else
    git worktree add -q --detach "$PAGES"
    git -C "$PAGES" checkout -q --orphan gh-pages
    git -C "$PAGES" rm -rq --cached . >/dev/null 2>&1 || true
    find "$PAGES" -mindepth 1 -maxdepth 1 ! -name .git -exec rm -rf {} +
fi
mkdir -p "$PAGES/api"
cp "$OUT/apps.json" "$PAGES/api/apps.json"
cp webos-app/icon130.png "$PAGES/icon.png"
: > "$PAGES/.nojekyll"
cat > "$PAGES/index.html" <<HTML
<!doctype html><meta charset="utf-8"><title>LG Hue Sync for the Homebrew Channel</title>
<p>Homebrew Channel repository for <a href="https://github.com/$REPO">LG Hue Sync</a>.
Add <code>$PAGES_URL/api/apps.json</code> in the Homebrew Channel's settings.</p>
HTML
git -C "$PAGES" add -A
if ! git -C "$PAGES" diff --cached --quiet; then
    git -C "$PAGES" commit -qm "Homebrew Channel repository for $TAG"
fi
git -C "$PAGES" push -q origin gh-pages

# GitHub may enable Pages by itself when gh-pages first appears (HTTP 409 then).
if ! gh api "repos/$REPO/pages" >/dev/null 2>&1; then
    gh api -X POST "repos/$REPO/pages" -f "source[branch]=gh-pages" -f "source[path]=/" >/dev/null 2>&1 \
        || gh api "repos/$REPO/pages" >/dev/null || die "could not enable GitHub Pages"
fi

echo "Published $TAG"
echo "  Release:    https://github.com/$REPO/releases/tag/$TAG"
echo "  Repository: $PAGES_URL/api/apps.json"
