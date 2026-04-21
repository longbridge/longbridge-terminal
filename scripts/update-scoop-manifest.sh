#!/usr/bin/env bash
# Update the Scoop manifest with the release version and SHA256 hash.
#
# Usage:
#   scripts/update-scoop-manifest.sh <version> <zip_path> [output_path]
#
# Arguments:
#   version      Release version without the "v" prefix (e.g. 0.18.0)
#   zip_path     Path to the Windows zip artifact
#   output_path  Destination for the updated manifest (default: longbridge.json)
#
# The script reads .scoop/longbridge.json as the template, updates the
# version, download URL and hash fields, and writes the result.

set -euo pipefail

VERSION="${1:-${VERSION:-}}"
ZIP_PATH="${2:-${ZIP_PATH:-}}"
OUTPUT_PATH="${3:-longbridge.json}"
TEMPLATE=".scoop/longbridge.json"

if [ -z "$VERSION" ] || [ -z "$ZIP_PATH" ]; then
    echo "Usage: $0 <version> <zip_path> [output_path]" >&2
    exit 1
fi

if [ ! -f "$ZIP_PATH" ]; then
    echo "ERROR: zip file not found: $ZIP_PATH" >&2
    exit 1
fi

SHA256=$(sha256sum "$ZIP_PATH" | awk '{print $1}')
URL="https://open.longbridge.cn/github/release/longbridge-terminal/v${VERSION}/longbridge-terminal-windows-amd64.zip"

jq \
    --arg version "$VERSION" \
    --arg url     "$URL" \
    --arg hash    "$SHA256" \
    '.version = $version
     | .architecture["64bit"].url  = $url
     | .architecture["64bit"].hash = $hash' \
    "$TEMPLATE" > "$OUTPUT_PATH"

echo "Scoop manifest written to $OUTPUT_PATH"
echo "  version : $VERSION"
echo "  url     : $URL"
echo "  hash    : $SHA256"
