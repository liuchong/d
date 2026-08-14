#!/usr/bin/env bash
# Shell example for syntax highlighting demo.
set -euo pipefail

PORT="${1:-8080}"

echo "Starting demo server on port ${PORT}..."

for file in *.rs; do
    size=$(wc -c < "$file")
    printf '  📄 %-20s %6d bytes\n' "$file" "$size"
done

echo "Done."
