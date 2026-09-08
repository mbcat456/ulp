#!/usr/bin/env sh
set -eu

repo="mbcat456/ulp"
asset="ulp-linux-x86_64.tar.gz"
dest="${ULP_INSTALL_DIR:-$HOME/.local/bin}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

arch="$(uname -m)"
if [ "$arch" != "x86_64" ] && [ "$arch" != "amd64" ]; then
  echo "ulp: unsupported architecture: $arch" >&2
  exit 1
fi

mkdir -p "$dest"
curl -fsSL "https://github.com/$repo/releases/latest/download/$asset" -o "$tmp/$asset"
tar -xzf "$tmp/$asset" -C "$dest"
chmod +x "$dest/ulp"
echo "Installed ulp to $dest/ulp"
