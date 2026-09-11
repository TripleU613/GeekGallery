#!/usr/bin/env python3
"""Verify the domain in Google Search Console and submit the sitemap, with a
service account and a DNS TXT record on Cloudflare.

Why a script: the Search Console "verify by DNS" flow is three manual steps in
two dashboards, and the sitemap has to be re-submitted whenever the property is
recreated. This does all of it idempotently, and can be re-run after the site
is live to confirm the sitemap is being read.

    export GOOGLE_SERVICE_ACCOUNT_JSON=/path/to/sa.json   # never in the repo
    export CF_API_EMAIL=... CF_GLOBAL_API_KEY=...          # to write the TXT record
    scripts/google-search-console.py [token|verify|gsc|all]

The service account's project needs the Site Verification API and the Search
Console API enabled (the script asks Service Usage to enable both). The
account that owns the Google Cloud project sees the property in the Search
Console UI once the service account has verified it and added them as an owner
-- or add yourself: sites.add with your own OAuth is the same call.

No credential is ever printed."""
import base64
import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

DOMAIN = os.environ["SITE_DOMAIN"]
# Looked up from the zone name rather than hardcoded, the same way
# cloudflare-provision.sh does it: an id in source is one more thing that is
# true for exactly one account.
ZONE_ID = os.environ.get("CF_ZONE_ID")
SA_PATH = os.environ.get("GOOGLE_SERVICE_ACCOUNT_JSON") or sys.exit("set GOOGLE_SERVICE_ACCOUNT_JSON")
sa = json.load(open(SA_PATH))


def b64(b):
    return base64.urlsafe_b64encode(b).rstrip(b"=").decode()


def token(scopes):
    """A service-account access token, signing the JWT with openssl so this has
    no dependency beyond the standard library and the openssl CLI."""
    now = int(time.time())
    hdr = b64(json.dumps({"alg": "RS256", "typ": "JWT"}).encode())
    claims = b64(json.dumps({"iss": sa["client_email"], "scope": " ".join(scopes),
                             "aud": sa["token_uri"], "iat": now, "exp": now + 3600}).encode())
    with tempfile.NamedTemporaryFile("w", suffix=".pem", delete=False) as kf:
        kf.write(sa["private_key"])
        kp = kf.name
    try:
        sig = subprocess.run(["openssl", "dgst", "-sha256", "-sign", kp],
                             input=f"{hdr}.{claims}".encode(), capture_output=True, check=True).stdout
    finally:
        os.unlink(kp)
    body = urllib.parse.urlencode({"grant_type": "urn:ietf:params:oauth:grant-type:jwt-bearer",
                                   "assertion": f"{hdr}.{claims}.{b64(sig)}"}).encode()
    r = urllib.request.urlopen(urllib.request.Request(
        sa["token_uri"], data=body, headers={"Content-Type": "application/x-www-form-urlencoded"}))
    return json.load(r)["access_token"]


def gapi(method, url, tok, body=None):
    req = urllib.request.Request(url, data=json.dumps(body).encode() if body is not None else None,
                                 method=method, headers={"Authorization": f"Bearer {tok}",
                                                         "Content-Type": "application/json"})
    try:
        r = urllib.request.urlopen(req)
        t = r.read().decode()
        return r.status, (json.loads(t) if t else {})
    except urllib.error.HTTPError as e:
        t = e.read().decode()
        try:
            return e.code, json.loads(t)
        except Exception:
            return e.code, {"raw": t[:300]}


def cfapi(method, path, body=None):
    req = urllib.request.Request("https://api.cloudflare.com/client/v4" + path,
                                 data=json.dumps(body).encode() if body else None, method=method,
                                 headers={"X-Auth-Email": os.environ["CF_API_EMAIL"],
                                          "X-Auth-Key": os.environ["CF_GLOBAL_API_KEY"],
                                          "Content-Type": "application/json"})
    return json.load(urllib.request.urlopen(req))

if not ZONE_ID:
    zones = cfapi("GET", f"/zones?name={DOMAIN}")["result"]
    if not zones:
        sys.exit(f"zone {DOMAIN} is not on this Cloudflare account")
    ZONE_ID = zones[0]["id"]


def short(d):
    return d.get("error", {}).get("message", d) if isinstance(d, dict) else d


step = sys.argv[1] if len(sys.argv) > 1 else "all"
tok = token(["https://www.googleapis.com/auth/siteverification",
             "https://www.googleapis.com/auth/webmasters",
             "https://www.googleapis.com/auth/cloud-platform"])
print("token: ok")

if step in ("all", "enable"):
    project = sa["client_id"] and sa["project_id"]
    for svc in ("siteverification.googleapis.com", "searchconsole.googleapis.com"):
        st, d = gapi("POST", f"https://serviceusage.googleapis.com/v1/projects/{project}/services/{svc}:enable", tok, {})
        print("enable", svc, st, "" if st == 200 else short(d))

if step in ("all", "token"):
    st, d = gapi("POST", "https://www.googleapis.com/siteVerification/v1/token", tok,
                 {"site": {"type": "INET_DOMAIN", "identifier": DOMAIN}, "verificationMethod": "DNS_TXT"})
    print("getToken:", st, short(d) if st != 200 else d.get("method"))
    if st == 200:
        txt = d["token"]
        recs = cfapi("GET", f"/zones/{ZONE_ID}/dns_records?type=TXT&name={DOMAIN}")["result"]
        if not any(r["content"].strip('"') == txt for r in recs):
            r = cfapi("POST", f"/zones/{ZONE_ID}/dns_records", {"type": "TXT", "name": DOMAIN, "content": txt, "ttl": 1})
            print("cloudflare TXT:", r["success"], r.get("errors"))
        else:
            print("cloudflare TXT: already present")

if step in ("all", "verify"):
    st, d = gapi("POST", "https://www.googleapis.com/siteVerification/v1/webResource?verificationMethod=DNS_TXT", tok,
                 {"site": {"type": "INET_DOMAIN", "identifier": DOMAIN}})
    print("verify:", st, short(d) if st != 200 else "owner: " + ",".join(d.get("owners", [])))

if step in ("all", "gsc"):
    prop = urllib.parse.quote(f"sc-domain:{DOMAIN}", safe="")
    st, d = gapi("PUT", f"https://www.googleapis.com/webmasters/v3/sites/{prop}", tok)
    print("sites.add:", st, short(d) if st not in (200, 204) else "ok")
    sm = urllib.parse.quote(f"https://{DOMAIN}/sitemap.xml", safe="")
    st, d = gapi("PUT", f"https://www.googleapis.com/webmasters/v3/sites/{prop}/sitemaps/{sm}", tok)
    print("sitemaps.submit:", st, short(d) if st not in (200, 204) else "ok")
    st, d = gapi("GET", f"https://www.googleapis.com/webmasters/v3/sites/{prop}", tok)
    print("site:", st, d)
