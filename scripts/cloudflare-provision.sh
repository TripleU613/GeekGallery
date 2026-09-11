#!/usr/bin/env bash
#
# Create everything one flavor needs on Cloudflare, idempotently, and print the
# infrastructure half of its deploy environment (see FLAVORS.md for the rest:
# the brand, the colours, the like button).
#
# WHAT IT MAKES, for a flavor named $FLAVOR on zone $CF_ZONE
#
#   D1 database        $FLAVOR               (the only database; see db.rs)
#   R2 bucket          $FLAVOR-media         + custom domain media.<zone>
#   API tokens         one D1-only, one R2-only -- so the app never holds a key
#                      that can touch anything else
#   Tunnel             $FLAVOR, with ingress to the app on loopback, and
#                      proxied CNAMEs for the apex and www
#   Redirect           www -> apex, 301, at the edge
#   Web Analytics      one site, returning the beacon token
#
# It does NOT apply the D1 migrations (see scripts/d1-migrate.sh) and it does not
# touch zone settings (see scripts/cloudflare-setup.sh). Three scripts, three
# jobs, each re-runnable on its own.
#
# USAGE
#
#   export CF_API_EMAIL=you@example.com
#   export CF_GLOBAL_API_KEY=...          # creating tokens and tunnels needs it
#   export CF_ZONE=example.com            # the site's zone, on this account
#   export FLAVOR=example                 # default: the zone's first label
#   scripts/cloudflare-provision.sh  > example.env   # never commit that file
#
# Everything is looked up before it is created, so a second run creates nothing
# and prints the same ids -- except the token *values*, which Cloudflare shows
# exactly once. Re-running mints new tokens; the old ones keep working until
# revoked in the dashboard, so nothing breaks, but rotate deliberately.
#
# No secret value is echoed to stderr, only to stdout, so `2>log` is safe to
# share and stdout is the one thing to protect.
set -euo pipefail

API=https://api.cloudflare.com/client/v4
ZONE_NAME=${CF_ZONE:?set CF_ZONE to the site's zone, e.g. example.com}
FLAVOR=${FLAVOR:-${ZONE_NAME%%.*}}
: "${CF_API_EMAIL:?set CF_API_EMAIL}"
: "${CF_GLOBAL_API_KEY:?set CF_GLOBAL_API_KEY}"
auth=(-H "X-Auth-Email: ${CF_API_EMAIL}" -H "X-Auth-Key: ${CF_GLOBAL_API_KEY}" -H 'Content-Type: application/json')

say() { echo "provision: $*" >&2; }
cf() { # method path [json]
  local m=$1 p=$2 body=${3:-}
  if [ -n "$body" ]; then
    curl -sS -X "$m" "${auth[@]}" "$API$p" --data "$body"
  else
    curl -sS -X "$m" "${auth[@]}" "$API$p"
  fi
}
# jq is not assumed; python is on every box this runs from.
jget() { python3 -c 'import json,sys; d=json.load(sys.stdin)
if not d.get("success", True): sys.stderr.write("cloudflare: %s\n" % d.get("errors")); sys.exit(1)
r=d.get("result")
for k in sys.argv[1].split("."):
    if k=="": continue
    if isinstance(r,list): r=r[int(k)]
    else: r=r.get(k) if isinstance(r,dict) else None
    if r is None: break
print(r if not isinstance(r,(dict,list)) else json.dumps(r))' "$1"; }

# ---- zone + account ---------------------------------------------------------
ZONE_JSON=$(cf GET "/zones?name=$ZONE_NAME")
ZONE_ID=$(echo "$ZONE_JSON" | jget 0.id)
ACCOUNT_ID=$(echo "$ZONE_JSON" | jget 0.account.id)
[ -n "$ZONE_ID" ] || { say "zone $ZONE_NAME is not on this account"; exit 1; }
say "zone $ZONE_NAME = $ZONE_ID (account $ACCOUNT_ID)"

# ---- D1 ---------------------------------------------------------------------
D1_NAME=$FLAVOR
D1_ID=$(cf GET "/accounts/$ACCOUNT_ID/d1/database?name=$D1_NAME" | python3 -c '
import json,sys; d=json.load(sys.stdin); m=[x for x in d.get("result",[]) if x["name"]==sys.argv[1]]; print(m[0]["uuid"] if m else "")' "$D1_NAME")
if [ -z "$D1_ID" ]; then
  D1_ID=$(cf POST "/accounts/$ACCOUNT_ID/d1/database" "{\"name\":\"$D1_NAME\"}" | jget uuid)
  say "created D1 database $D1_NAME"
else
  say "D1 database $D1_NAME exists"
fi

# ---- R2 ---------------------------------------------------------------------
BUCKET=$FLAVOR-media
if cf GET "/accounts/$ACCOUNT_ID/r2/buckets/$BUCKET" | python3 -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("success") else 1)'; then
  say "R2 bucket $BUCKET exists"
else
  cf POST "/accounts/$ACCOUNT_ID/r2/buckets" "{\"name\":\"$BUCKET\"}" | jget name >/dev/null
  say "created R2 bucket $BUCKET"
fi
MEDIA_HOST="media.$ZONE_NAME"
if cf GET "/accounts/$ACCOUNT_ID/r2/buckets/$BUCKET/domains/custom/$MEDIA_HOST" | python3 -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("success") else 1)'; then
  say "R2 custom domain $MEDIA_HOST attached"
else
  cf POST "/accounts/$ACCOUNT_ID/r2/buckets/$BUCKET/domains/custom" \
    "{\"domain\":\"$MEDIA_HOST\",\"zoneId\":\"$ZONE_ID\",\"enabled\":true,\"minTLS\":\"1.2\"}" | jget domain >/dev/null
  say "attached $MEDIA_HOST to $BUCKET (Cloudflare manages the DNS record)"
fi

# ---- scoped tokens ----------------------------------------------------------
# Permission group ids are looked up by name rather than hardcoded: they are
# stable in practice but documented nowhere as a contract.
PG=$(cf GET /user/tokens/permission_groups)
pg_id() { echo "$PG" | python3 -c 'import json,sys; d=json.load(sys.stdin); print([x["id"] for x in d["result"] if x["name"]==sys.argv[1]][0])' "$1"; }
D1_WRITE=$(pg_id "D1 Write")
R2_WRITE=$(pg_id "Workers R2 Storage Write")

mint() { # name permission-group-id
  cf POST /user/tokens "{\"name\":\"$1\",\"policies\":[{\"effect\":\"allow\",\"resources\":{\"com.cloudflare.api.account.$ACCOUNT_ID\":\"*\"},\"permission_groups\":[{\"id\":\"$2\"}]}]}"
}
D1_TOKEN_JSON=$(mint "$FLAVOR d1 ($(date -u +%Y-%m-%d))" "$D1_WRITE")
CF_D1_API_TOKEN=$(echo "$D1_TOKEN_JSON" | jget value)
say "minted D1 token $(echo "$D1_TOKEN_JSON" | jget id)"

R2_TOKEN_JSON=$(mint "$FLAVOR r2 ($(date -u +%Y-%m-%d))" "$R2_WRITE")
# R2's S3 API: the access key id is the token's id and the secret is the
# SHA-256 of the token value. Documented by Cloudflare, not something guessed.
R2_ACCESS_KEY_ID=$(echo "$R2_TOKEN_JSON" | jget id)
R2_SECRET_ACCESS_KEY=$(echo "$R2_TOKEN_JSON" | jget value | tr -d '\n' | sha256sum | cut -d' ' -f1)
say "minted R2 token $R2_ACCESS_KEY_ID"

# ---- tunnel -----------------------------------------------------------------
TUNNEL_NAME=$FLAVOR
TUNNEL_ID=$(cf GET "/accounts/$ACCOUNT_ID/cfd_tunnel?name=$TUNNEL_NAME&is_deleted=false" | python3 -c '
import json,sys; d=json.load(sys.stdin); m=[x for x in d.get("result",[]) if x["name"]==sys.argv[1]]; print(m[0]["id"] if m else "")' "$TUNNEL_NAME")
if [ -z "$TUNNEL_ID" ]; then
  TUNNEL_ID=$(cf POST "/accounts/$ACCOUNT_ID/cfd_tunnel" "{\"name\":\"$TUNNEL_NAME\",\"config_src\":\"cloudflare\"}" | jget id)
  say "created tunnel $TUNNEL_NAME"
else
  say "tunnel $TUNNEL_NAME exists"
fi
CF_TUNNEL_TOKEN=$(cf GET "/accounts/$ACCOUNT_ID/cfd_tunnel/$TUNNEL_ID/token" | jget "")
# Ingress: the app listens on loopback inside the same container as cloudflared
# (see deploy/supervisord.conf), so the service is 127.0.0.1, not a hostname.
cf PUT "/accounts/$ACCOUNT_ID/cfd_tunnel/$TUNNEL_ID/configurations" "{\"config\":{\"ingress\":[
  {\"hostname\":\"$ZONE_NAME\",\"service\":\"http://127.0.0.1:3100\"},
  {\"hostname\":\"www.$ZONE_NAME\",\"service\":\"http://127.0.0.1:3100\"},
  {\"service\":\"http_status:404\"}]}}" | jget tunnel_id >/dev/null
say "tunnel ingress set"

# ---- DNS --------------------------------------------------------------------
dns_upsert() { # name content
  local existing
  existing=$(cf GET "/zones/$ZONE_ID/dns_records?name=$1&type=CNAME" | python3 -c 'import json,sys; r=json.load(sys.stdin)["result"]; print(r[0]["id"] if r else "")')
  local body="{\"type\":\"CNAME\",\"name\":\"$1\",\"content\":\"$2\",\"proxied\":true,\"ttl\":1}"
  if [ -n "$existing" ]; then cf PUT "/zones/$ZONE_ID/dns_records/$existing" "$body" | jget id >/dev/null
  else cf POST "/zones/$ZONE_ID/dns_records" "$body" | jget id >/dev/null; fi
  say "dns $1 -> $2 (proxied)"
}
dns_upsert "$ZONE_NAME" "$TUNNEL_ID.cfargotunnel.com"
dns_upsert "www.$ZONE_NAME" "$TUNNEL_ID.cfargotunnel.com"

# ---- www -> apex ------------------------------------------------------------
# One canonical host. The app already emits canonical tags, but a 301 at the
# edge is what stops the www copy being crawled at all.
cf PUT "/zones/$ZONE_ID/rulesets/phases/http_request_dynamic_redirect/entrypoint" "{\"rules\":[{
  \"description\":\"www to apex\",
  \"expression\":\"(http.host eq \\\"www.$ZONE_NAME\\\")\",
  \"action\":\"redirect\",
  \"action_parameters\":{\"from_value\":{\"status_code\":301,\"preserve_query_string\":true,
    \"target_url\":{\"expression\":\"concat(\\\"https://$ZONE_NAME\\\", http.request.uri.path)\"}}},
  \"enabled\":true}]}" | jget id >/dev/null
say "redirect www -> apex"

# ---- web analytics ----------------------------------------------------------
CF_ANALYTICS_TOKEN=$(cf GET "/accounts/$ACCOUNT_ID/rum/site_info/list" | python3 -c '
import json,sys,re; d=json.load(sys.stdin)
for s in d.get("result",[]):
    if s.get("host")==sys.argv[1] or s.get("zone_tag")==sys.argv[2]:
        m=re.search(r"\"token\": ?\"([0-9a-f]+)\"", s.get("snippet","")); print(m.group(1) if m else ""); break' "$ZONE_NAME" "$ZONE_ID")
if [ -z "$CF_ANALYTICS_TOKEN" ]; then
  CF_ANALYTICS_TOKEN=$(cf POST "/accounts/$ACCOUNT_ID/rum/site_info" "{\"host\":\"$ZONE_NAME\",\"zone_tag\":\"$ZONE_ID\",\"auto_install\":false}" | python3 -c '
import json,sys,re; d=json.load(sys.stdin); m=re.search(r"\"token\": ?\"([0-9a-f]+)\"", d["result"].get("snippet","")); print(m.group(1) if m else "")')
  say "created Web Analytics site"
else
  say "Web Analytics site exists"
fi

# ---- the answer -------------------------------------------------------------
# The infrastructure half of the flavor's deploy environment. Fill in the
# blanks, add the SITE_*/THEME_* lines from FLAVORS.md, and store it as the
# broker secret FLAVOR_ENV_<NAME> (deploy/env-broker/README.md).
cat <<OUT
# $FLAVOR -- one flavor's deploy environment. Generated $(date -u +%Y-%m-%dT%H:%M:%SZ). Do not commit.
SITE_ORIGIN=https://$ZONE_NAME
SITE_NAME=$FLAVOR
SITE_NOUN=item
CF_ACCOUNT_ID=$ACCOUNT_ID
CF_D1_DATABASE_ID=$D1_ID
CF_D1_API_TOKEN=$CF_D1_API_TOKEN
R2_ACCESS_KEY_ID=$R2_ACCESS_KEY_ID
R2_SECRET_ACCESS_KEY=$R2_SECRET_ACCESS_KEY
R2_BUCKET=$BUCKET
R2_PUBLIC_BASE=https://$MEDIA_HOST
CF_TUNNEL_TOKEN=$CF_TUNNEL_TOKEN
CF_ANALYTICS_TOKEN=$CF_ANALYTICS_TOKEN
SESSION_SECRET=$(openssl rand -base64 48 | tr -d '\n')
GOOGLE_CLIENT_ID=
GOOGLE_CLIENT_SECRET=
ADMIN_EMAILS=
PSEUDONYMS=
INDEXNOW_KEY=$(openssl rand -hex 16)
TURNSTILE_SITE_KEY=
TURNSTILE_SECRET=
DEPLOY_HOST=
TS_CI_AUTHKEY=
OUT
say "done"
