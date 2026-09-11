#!/usr/bin/env bash
#
# Apply every Cloudflare zone setting worth setting on the free plan for
# one flavor's zone.
#
# WHY A SCRIPT AND NOT A DASHBOARD CHECKLIST
#
# Half of these settings are load-bearing for this specific app and three of them
# will break it outright if someone turns them on later "because they sound like
# performance". A script says which and why, lives in the repo, and can be re-run
# after any dashboard fiddling to put the zone back. It is idempotent: every call
# is a PATCH to a desired state, so running it twice changes nothing the second
# time.
#
# USAGE
#
#   scripts/cloudflare-setup.sh                 # dry run, prints what would change
#   scripts/cloudflare-setup.sh --apply         # actually writes
#
# Needs a token that the D1 token in .env is NOT: create one at
# dash.cloudflare.com/profile/api-tokens with
#   Zone / Zone Settings / Edit
#   Zone / Cache Rules  / Edit      (only for the cache-rule section)
#   Zone / Zone         / Read
# scoped to this one zone, then:
#
#   export CF_ZONE_API_TOKEN=...
#   export CF_ZONE=example.com          # or CF_ZONE_ID=...
#
# Settings the free plan does not include are reported as unavailable and
# skipped rather than failing the run — that is the point of reporting each
# call's own answer instead of assuming a plan tier.

set -uo pipefail

API=https://api.cloudflare.com/client/v4
ZONE_NAME=${CF_ZONE:?set CF_ZONE to the zone, e.g. example.com}
APPLY=0
[[ ${1:-} == --apply ]] && APPLY=1

# Either credential form works, because "the global token" means two different
# things depending on which page of the dashboard you got it from:
#
#   CF_ZONE_API_TOKEN                     a scoped API token   -> Bearer
#   CF_API_EMAIL + CF_GLOBAL_API_KEY      the legacy Global Key -> X-Auth-*
#
# The token in .env (CF_D1_API_TOKEN) is neither: it verifies as active but is
# scoped to D1 alone — it lists zero zones and cannot read the account — so it
# cannot touch zone settings. Checked, not assumed.
if [[ -n ${CF_ZONE_API_TOKEN:-} ]]; then
  auth=(-H "Authorization: Bearer ${CF_ZONE_API_TOKEN}" -H 'Content-Type: application/json')
elif [[ -n ${CF_GLOBAL_API_KEY:-} && -n ${CF_API_EMAIL:-} ]]; then
  auth=(-H "X-Auth-Email: ${CF_API_EMAIL}" -H "X-Auth-Key: ${CF_GLOBAL_API_KEY}"
    -H 'Content-Type: application/json')
else
  cat >&2 <<'MISSING'
No zone-capable credential in the environment. Set either:

  export CF_ZONE_API_TOKEN=...                       # scoped token (preferred)

or, for the legacy Global API Key:

  export CF_API_EMAIL=you@example.com
  export CF_GLOBAL_API_KEY=...

CF_D1_API_TOKEN from .env will not work — it is scoped to D1 and lists no zones.
MISSING
  exit 1
fi

ZONE_ID=${CF_ZONE_ID:-}
if [[ -z $ZONE_ID ]]; then
  ZONE_ID=$(curl -s "${auth[@]}" "$API/zones?name=$ZONE_NAME" |
    python3 -c 'import json,sys; r=json.load(sys.stdin).get("result") or []; print(r[0]["id"] if r else "")')
fi
if [[ -z $ZONE_ID ]]; then
  echo "Could not resolve a zone id for '$ZONE_NAME'. Is the token scoped to this zone?" >&2
  exit 1
fi

PLAN=$(curl -s "${auth[@]}" "$API/zones/$ZONE_ID" |
  python3 -c 'import json,sys; print(((json.load(sys.stdin).get("result") or {}).get("plan") or {}).get("name","?"))')
echo "zone $ZONE_NAME ($ZONE_ID) — plan: $PLAN"
(( APPLY )) || echo "DRY RUN. Re-run with --apply to write."
echo

pass=0 skip=0 fail=0

# setting <id> <json-value> <why>
setting() {
  local id=$1 value=$2 why=$3
  local current
  current=$(curl -s "${auth[@]}" "$API/zones/$ZONE_ID/settings/$id" |
    python3 -c 'import json,sys
d=json.load(sys.stdin)
if not d.get("success"):
    errs=d.get("errors") or [{}]
    print("UNAVAILABLE:"+str(errs[0].get("message","?"))); raise SystemExit
print(json.dumps((d.get("result") or {}).get("value")))' 2>/dev/null)

  if [[ $current == UNAVAILABLE:* ]]; then
    printf '  %-28s skip      (%s)\n' "$id" "${current#UNAVAILABLE:}"
    (( skip++ )); return
  fi

  if [[ $current == "$value" ]]; then
    printf '  %-28s ok        already %s\n' "$id" "$value"
    (( pass++ )); return
  fi

  if (( ! APPLY )); then
    printf '  %-28s WOULD SET %s -> %s\n' "$id" "${current:-?}" "$value"
    printf '  %-28s           %s\n' '' "$why"
    (( pass++ )); return
  fi

  local out
  out=$(curl -s -X PATCH "${auth[@]}" "$API/zones/$ZONE_ID/settings/$id" \
    --data "{\"value\":$value}" |
    python3 -c 'import json,sys
d=json.load(sys.stdin)
print("ok" if d.get("success") else "FAIL "+str((d.get("errors") or [{}])[0].get("message","?")))')
  if [[ $out == ok ]]; then
    printf '  %-28s SET       -> %s\n' "$id" "$value"
    (( pass++ ))
  else
    printf '  %-28s %s\n' "$id" "$out"
    (( fail++ ))
  fi
}

echo "— Correctness: these three break this app if enabled —"
# Rocket Loader rewrites and defers script execution. Hydration is loaded as a
# native ES module (`<script type="module">` from HydrationScripts) and the wasm
# is fetched by it; deferring or reordering that is how you get a page that
# renders and never becomes interactive.
setting rocket_loader '"off"' 'reorders/defers scripts; kills ES-module wasm hydration'
# Injects a script into every HTML response to obfuscate mailto: links. There are
# no email addresses on the site, so this is pure injected JS and one more thing
# that can differ between the server HTML and the hydrated DOM.
setting email_obfuscation '"off"' 'injects JS into HTML for zero benefit here'
# Strips content between <!--sse--> comments based on visitor reputation, which
# means an HTML response that varies invisibly under a framework that hydrates
# against exactly the HTML it sent.
setting server_side_exclude '"off"' 'mutates server HTML; hydration must match'

echo
echo "— Caching: origin already sends the right headers, so do not override them —"
# 0 == "Respect Existing Headers". main.rs sets these deliberately: HTML is
# private/no-cache because SSR embeds per-visitor state (liked_by_me, the account
# menu), and /pkg/ assets are content-hashed and immutable. Any TTL set here
# would override that and is how you serve one visitor's session state to
# another.
setting browser_cache_ttl '0' 'respect origin Cache-Control; HTML is per-visitor'
setting cache_level '"aggressive"' 'standard caching, honouring origin headers'
# Never leave this on. It bypasses cache globally and silently.
setting development_mode '"off"' 'bypasses all caching'
# Lets Cloudflare serve a stale copy if the origin is unreachable. This origin is
# a home server behind a Cloudflare Tunnel, so "unreachable" is a realistic
# Tuesday rather than a hypothetical.
setting always_online '"on"' 'serve stale on origin outage; origin is a tunnelled box'

echo
echo "— Transport and compression —"
setting brotli '"on"' 'compress text at the edge'
setting early_hints '"on"' '103 Early Hints for the CSS link'
setting http3 '"on"' 'HTTP/3'
setting zero_rtt '"on"' '0-RTT resumption for repeat visitors'
setting ipv6 '"on"' 'IPv6'
setting opportunistic_encryption '"on"' 'encrypt even plaintext-negotiated requests'
# Nothing here needs WebSockets today; leaving them on costs nothing.
setting websockets '"on"' 'harmless, and cheaper than finding out something needs it'

echo
echo "— TLS —"
# Full (strict): the origin is reached through a Cloudflare Tunnel, which
# presents a real Cloudflare-issued certificate, so there is no reason to accept
# anything weaker.
setting ssl '"strict"' 'origin is a Cloudflare Tunnel; verify it'
setting min_tls_version '"1.2"' 'drop TLS 1.0/1.1'
setting tls_1_3 '"on"' 'TLS 1.3'
setting always_use_https '"on"' 'redirect http -> https at the edge'
setting automatic_https_rewrites '"on"' 'rewrite subresource http:// links'

echo
echo "— SEO —"
# Tells Cloudflare to hint crawlers when content changes, so a new item is
# discovered without waiting for a scheduled recrawl.
setting crawler_hints '"on"' 'notify crawlers on change'

echo
echo "— Security: deliberately NOT locked down further —"
setting security_level '"medium"' 'default; see the note below on Bot Fight Mode'
# Hotlink protection is left OFF on purpose and is not set here: /embed and the
# oEmbed endpoint exist so other sites CAN hotlink an item, and R2 media is served
# cross-origin by design. Turning it on breaks the embed feature.

cat <<'NOTE'

Two things this script deliberately does not turn on
---------------------------------------------------

Bot Fight Mode (free) injects a JS challenge into responses to clients it judges
automated. Discord, Slack, Twitter and Google's unfurlers ARE automated clients,
and unfurling is the product: this whole app is SsrMode::Async specifically so
that og: tags reach them in the first response. Enabling it trades link previews
for bot noise. Leave it off, and if scraping ever becomes a real cost, rate-limit
/api/* by path instead.

Hotlink Protection, for the same class of reason: /embed/:id and /oembed exist so
other sites can embed an item, and the media host is a public image host
by design.

Cache Rules worth adding by hand (free plan allows 10)
------------------------------------------------------

The origin headers already do the right thing, so these are belt-and-braces
rather than load-bearing — add them only if you want the edge to enforce it
independently of the app:

  1. Bypass cache   when  URI Path starts with  /api/ or /auth/ or /admin
  2. Bypass cache   when  URI Path ends with    /download
  3. Eligible for cache, Edge TTL "respect origin", when URI Path starts with /pkg/

Do NOT add a rule that caches "/" or the item pages at the edge. Those responses embed
per-visitor state — whether you have liked an item, and your account menu — so
an edge-cached copy serves one visitor's session to the next.
NOTE

echo
echo "summary: $pass ok/would-set, $skip unavailable on this plan, $fail failed"
(( fail == 0 ))
