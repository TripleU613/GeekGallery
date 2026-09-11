#!/usr/bin/env bash
# Apply every migration in migrations/ that this database has not seen, in
# order, remotely.
#
# Through wrangler rather than the raw D1 HTTP API: wrangler splits a file into
# statements correctly, which matters here because 0005 and 0007 create
# triggers whose bodies contain semicolons -- a naive split on `;` sends half a
# trigger and D1 rejects it.
#
# Applied migrations are recorded in `_migrations` by file name, so this is
# safe to run on every deploy and against any database: a fresh one gets all
# of them, an existing one gets only what it has not seen.
#
#   export CLOUDFLARE_ACCOUNT_ID=...
#   export CLOUDFLARE_API_TOKEN=...      # a D1 Write token; or CLOUDFLARE_API_KEY + CLOUDFLARE_EMAIL
#   scripts/d1-migrate.sh <database-name>
#
# Needs Node for `npx wrangler`. Nothing else in this repo does.
set -euo pipefail
cd "$(dirname "$0")/.."

DB=${1:?usage: d1-migrate.sh <database-name>}
export WRANGLER_SEND_METRICS=false

# wrangler wants a config file to find the binding even when the database is
# named on the command line; a throwaway one in a temp dir keeps the repo clean.
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cat > "$tmp/wrangler.toml" <<TOML
name = "geekgallery-migrate"
compatibility_date = "2026-01-01"
[[d1_databases]]
binding = "DB"
database_name = "$DB"
database_id = "${CF_D1_DATABASE_ID:-00000000-0000-0000-0000-000000000000}"
TOML

wr() { npx --yes wrangler@4 d1 execute "$DB" --remote -y -c "$tmp/wrangler.toml" "$@"; }

wr --command "CREATE TABLE IF NOT EXISTS _migrations (name TEXT PRIMARY KEY NOT NULL, applied_at TEXT NOT NULL)" >/dev/null
applied=$(wr --json --command "SELECT name FROM _migrations" \
  | python3 -c 'import json,sys; print("\n".join(r["name"] for r in json.load(sys.stdin)[0]["results"]))')

for f in migrations/*.sql; do
  name=$(basename "$f")
  if grep -qx "$name" <<<"$applied"; then
    continue
  fi
  echo "d1-migrate: $name"
  wr --file "$f" >/dev/null
  wr --command "INSERT INTO _migrations (name, applied_at) VALUES ('$name', datetime('now'))" >/dev/null
done
echo "d1-migrate: done ($DB)"
