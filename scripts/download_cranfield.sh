#!/usr/bin/env bash
# Downloads the Cranfield collection (1,400 docs, 225 queries, relevance judgements)
# from the University of Glasgow IR test collections into data/cranfield/.
set -euo pipefail

URL="https://ir.dcs.gla.ac.uk/resources/test_collections/cran/cran.tar.gz"
SHA256="8c48a8412e4a7e6e0dd5af99c959f8d75fdf88135602adc1a56aa22770652d64"
DEST="${1:-data/cranfield}"

mkdir -p "$DEST"
curl --fail --silent --show-error --location "$URL" --output "$DEST/cran.tar.gz"

if command -v sha256sum >/dev/null; then
  actual=$(sha256sum "$DEST/cran.tar.gz" | cut -d' ' -f1)
else
  actual=$(shasum -a 256 "$DEST/cran.tar.gz" | cut -d' ' -f1)
fi
if [[ "$actual" != "$SHA256" ]]; then
  echo "checksum mismatch: expected $SHA256, got $actual" >&2
  exit 1
fi

tar -xzf "$DEST/cran.tar.gz" -C "$DEST"
ls -l "$DEST"
