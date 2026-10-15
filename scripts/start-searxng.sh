#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
state_dir="$repo_root/state"
secret_file="$state_dir/searxng.env"

mkdir -p "$state_dir"
if [[ ! -s "$secret_file" ]]; then
  python3 -c 'import secrets; print("SEARXNG_SECRET=" + secrets.token_urlsafe(48))' > "$secret_file"
  chmod 600 "$secret_file"
fi

docker compose -f "$repo_root/compose.searxng.yml" up -d
