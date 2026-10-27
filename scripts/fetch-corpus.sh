#!/usr/bin/env bash
# Download the third-party test corpus listed in testdata/corpus.tsv into
# testdata/corpus/<source>/. Files are pinned to upstream commits and verified
# against the SHA-256 recorded in the manifest.
#
# Usage:
#   scripts/fetch-corpus.sh          download missing files and verify all
#   scripts/fetch-corpus.sh --pin    (maintainers) also record hashes for new entries
#
# Works with the stock bash 3.2 on macOS, Linux, and Git Bash on Windows.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
manifest="$root/testdata/corpus.tsv"
dest="$root/testdata/corpus"
pin=0
[ "${1:-}" = "--pin" ] && pin=1

base_url() {
  case "$1" in
    tika) echo "https://raw.githubusercontent.com/apache/tika/288e486531dcb44be1192f0524d6bdca1a7c0a28/tika-parsers/tika-parsers-standard/tika-parsers-standard-modules/tika-parser-microsoft-module/src/test/resources/test-documents" ;;
    poi) echo "https://raw.githubusercontent.com/apache/poi/2c00b85db18148bca97449f1886ded08be100ffc/test-data/diagram" ;;
    libvisio) echo "https://raw.githubusercontent.com/LibreOffice/libvisio/a42aaff13e4a67afcd077c3abb3ef13bb6463025/src/test/data" ;;
    tika-pdf) echo "https://raw.githubusercontent.com/apache/tika/288e486531dcb44be1192f0524d6bdca1a7c0a28/tika-parsers/tika-parsers-standard/tika-parsers-standard-modules/tika-parser-pdf-module/src/test/resources/test-documents" ;;
    pdfjs) echo "https://raw.githubusercontent.com/mozilla/pdf.js/5279fe0db4374d509a9beb169a7869ce00be7436/test/pdfs" ;;
    *) echo "unknown corpus source: $1" >&2; return 1 ;;
  esac
}

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

fetched=0
verified=0
failed=0
updated_manifest="$(mktemp)"
trap 'rm -f "$updated_manifest"' EXIT

while IFS= read -r line || [ -n "$line" ]; do
  case "$line" in
    '#'* | '')
      printf '%s\n' "$line" >>"$updated_manifest"
      continue
      ;;
  esac
  source="$(printf '%s' "$line" | cut -f1)"
  file="$(printf '%s' "$line" | cut -f2)"
  expected="$(printf '%s' "$line" | cut -s -f3)"
  target="$dest/$source/$file"
  mkdir -p "$dest/$source"

  if [ ! -f "$target" ] || { [ -n "$expected" ] && [ "$(sha256 "$target")" != "$expected" ]; }; then
    url="$(base_url "$source")/$file"
    if ! curl -fsSL --retry 3 --retry-delay 2 -o "$target.part" "$url"; then
      echo "error: download failed: $url" >&2
      rm -f "$target.part"
      failed=$((failed + 1))
      printf '%s\n' "$line" >>"$updated_manifest"
      continue
    fi
    mv "$target.part" "$target"
    fetched=$((fetched + 1))
  fi

  actual="$(sha256 "$target")"
  if [ -z "$expected" ]; then
    if [ "$pin" -eq 1 ]; then
      printf '%s\t%s\t%s\n' "$source" "$file" "$actual" >>"$updated_manifest"
    else
      echo "warning: no pinned hash for $source/$file (run with --pin)" >&2
      printf '%s\n' "$line" >>"$updated_manifest"
    fi
  elif [ "$actual" != "$expected" ]; then
    echo "error: checksum mismatch for $source/$file" >&2
    echo "  expected $expected" >&2
    echo "  actual   $actual" >&2
    failed=$((failed + 1))
    printf '%s\n' "$line" >>"$updated_manifest"
  else
    verified=$((verified + 1))
    printf '%s\n' "$line" >>"$updated_manifest"
  fi
done <"$manifest"

if [ "$pin" -eq 1 ]; then
  cp "$updated_manifest" "$manifest"
fi

echo "corpus: $fetched downloaded, $verified verified, $failed failed ($dest)"
[ "$failed" -eq 0 ]
