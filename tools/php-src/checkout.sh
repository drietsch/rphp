#!/usr/bin/env bash
# Sparse, shallow checkout of the php-src `.phpt` corpus at the pinned tag.
#
# Usage: tools/php-src/checkout.sh [<dir>]
#   <dir>   target directory (default: <repo>/vendor-php-src, git-ignored)
#
# Only the test trees are materialised (`ext/*/tests`, `Zend/tests`, `tests`,
# `sapi/cli/tests`) plus `run-tests.php` for reference. Re-running is a no-op
# when the checkout already sits on the pin, otherwise it fast-forwards to it.
# The pin lives in tools/php-src/PIN; `cargo xtask fetch-php-src` wraps this.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$here/../.." && pwd)"
pin="$(tr -d '[:space:]' < "$here/PIN")"
dir="${1:-${PHP_SRC_DIR:-$repo_root/vendor-php-src}}"
url="${PHP_SRC_URL:-https://github.com/php/php-src}"

if ! command -v git >/dev/null 2>&1; then
    echo "checkout.sh: git is not installed or not on PATH; install git (https://git-scm.com) and re-run." >&2
    exit 2
fi

sparse_paths=(
    '/ext/*/tests/'
    '/Zend/tests/'
    '/tests/'
    '/sapi/cli/tests/'
    '/run-tests.php'
)

if [ -d "$dir/.git" ]; then
    current="$(git -C "$dir" describe --tags --exact-match HEAD 2>/dev/null || true)"
    if [ "$current" = "$pin" ]; then
        echo "php-src already at $pin in $dir"
        exit 0
    fi
    echo "php-src: moving $dir from '${current:-?}' to $pin"
    git -C "$dir" fetch --depth 1 --filter=blob:none origin "refs/tags/$pin:refs/tags/$pin"
    git -C "$dir" sparse-checkout set --no-cone "${sparse_paths[@]}"
    git -C "$dir" checkout --quiet --force "$pin"
    echo "php-src now at $pin in $dir"
    exit 0
fi

echo "php-src: sparse shallow clone of $url @ $pin into $dir"
git clone --quiet --filter=blob:none --no-checkout --depth 1 --branch "$pin" "$url" "$dir"
git -C "$dir" sparse-checkout set --no-cone "${sparse_paths[@]}"
git -C "$dir" checkout --quiet "$pin"
echo "php-src now at $pin in $dir"
