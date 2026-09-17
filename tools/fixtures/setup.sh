#!/usr/bin/env bash
# Install the Composer dependencies of every ladder fixture with the *stock*
# php (the platform check must pass there; rphp then has to pass it itself).
# vendor/ and var/ are git-ignored; composer.json + composer.lock are committed.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
php_bin="${PHP_BIN:-php}"
for dir in "$root"/fixtures/ladder/*/; do
  [ -f "$dir/composer.json" ] || continue
  echo "== $(basename "$dir")"
  (cd "$dir" && "$php_bin" "$(command -v composer)" install --no-interaction --no-progress --prefer-dist "$@")
done
