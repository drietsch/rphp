#!/usr/bin/env bash
# Seed the fuzz corpora from the repo's PHP snippets so libFuzzer starts from
# real syntax instead of random bytes.
#
#   fuzz/seed-corpus.sh            # seeds corpus/parse_bytes and corpus/tokenize_bytes
#   fuzz/seed-corpus.sh <dir>...   # additionally pull *.php from these dirs
#
# Sources: examples/**/*.php, crates/rphp-tokenizer/tests/**/*.php (the
# tokenizer differential corpus, once F5 lands), and any extra directories.
# Files are content-addressed (sha256) so reseeding never duplicates.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"

sources=("$repo/examples" "$repo/crates/rphp-tokenizer/tests" "$@")
targets=(parse_bytes tokenize_bytes)

hash_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

for t in "${targets[@]}"; do
    mkdir -p "$here/corpus/$t"
done

count=0
for src in "${sources[@]}"; do
    [ -d "$src" ] || continue
    while IFS= read -r -d '' f; do
        h="$(hash_file "$f")"
        for t in "${targets[@]}"; do
            cp -f "$f" "$here/corpus/$t/$h"
        done
        count=$((count + 1))
    done < <(find "$src" -type f -name '*.php' -print0)
done

echo "seeded ${#targets[@]} corpora with $count file(s) under $here/corpus/"
