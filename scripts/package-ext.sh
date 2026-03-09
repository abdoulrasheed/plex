#!/usr/bin/env bash
set -euo pipefail

# Build a platform-specific VSIX for the current machine.
# Usage: ./scripts/package-ext.sh

cd "$(dirname "$0")/.."

echo "==> Building plex binary (release)..."
cargo build --release

echo "==> Copying binary into extension..."
mkdir -p vscode-plex/bin

if [[ "$OSTYPE" == "msys" || "$OSTYPE" == "cygwin" ]]; then
    cp target/release/plex.exe vscode-plex/bin/plex.exe
else
    cp target/release/plex vscode-plex/bin/plex
    chmod +x vscode-plex/bin/plex
fi

echo "==> Installing extension dependencies..."
cd vscode-plex
npm ci --ignore-scripts 2>/dev/null || npm install

echo "==> Packaging VSIX..."
npx @vscode/vsce package

echo ""
echo "Done! VSIX is in vscode-plex/"
ls -lh *.vsix
