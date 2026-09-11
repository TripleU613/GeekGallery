// The deploy-env broker.
//
// GitHub Actions can prove who it is without any shared secret: every job can
// mint an OIDC token, signed by GitHub, that names the repository, the ref and
// the event that started it. This Worker checks that signature against
// GitHub's published keys, checks that the claims say "a push to main of the
// geekgallery repository", and only then answers out of its own secrets.
//
// Two requests, both POST with the token as a Bearer:
//
//   /flavors          -> JSON: the names of every flavor this broker holds
//   /flavor/<name>    -> that flavor's whole deploy environment, KEY=VALUE
//
// A flavor is one Worker secret, FLAVOR_ENV_<NAME> (upper case, so the flavor
// "example" is FLAVOR_ENV_EXAMPLE), holding the whole file for one
// deployment: its Cloudflare credentials, its Google client, its brand, its
// colours, its mirror, its deploy host. Adding a site to the fleet is
// `wrangler secret put FLAVOR_ENV_NEWSITE < newsite.env` and the next push
// deploys it; there is no list to keep in step, because /flavors is read off
// the bindings themselves. Secrets on Cloudflare carry no size ceiling worth
// worrying about here (a full flavor is a few kilobytes), which is why they
// are blobs and not one secret per value.
//
// It exists because it is a better home for the values than forty GitHub
// secrets: one place on the same Cloudflare account as the resources the
// values point at, rotated with `wrangler secret put`, readable by nothing
// but a signed push to main.
//
// Anything not exactly right is a 403 with no body. There is nothing to
// enumerate here without a valid token.

const ISSUER = "https://token.actions.githubusercontent.com";
const JWKS_URL = `${ISSUER}/.well-known/jwks`;

const REF = "refs/heads/main";
// A re-run of a failed deploy is still a push; workflow_dispatch is allowed
// so a deploy can be kicked by hand from the Actions tab.
const EVENTS = new Set(["push", "workflow_dispatch"]);

// The one audience and the one repository allowed to use it. `REPOSITORY` is
// a plain var in wrangler.toml so a fork can point the broker at itself.
const AUDIENCE = "geekgallery-deploy";

const FLAVOR_PREFIX = "FLAVOR_ENV_";

let jwksCache = { at: 0, keys: [] };

async function jwks() {
  if (Date.now() - jwksCache.at < 10 * 60 * 1000 && jwksCache.keys.length) {
    return jwksCache.keys;
  }
  const r = await fetch(JWKS_URL, { cf: { cacheTtl: 600 } });
  if (!r.ok) throw new Error(`jwks ${r.status}`);
  const { keys } = await r.json();
  jwksCache = { at: Date.now(), keys };
  return keys;
}

function b64url(s) {
  s = s.replace(/-/g, "+").replace(/_/g, "/");
  while (s.length % 4) s += "=";
  return Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
}

// The verified claims, or null. Checks the signature first and the claims
// second, so an unsigned token learns nothing from which check it failed.
async function verify(token) {
  const parts = token.split(".");
  if (parts.length !== 3) return null;
  const header = JSON.parse(new TextDecoder().decode(b64url(parts[0])));
  const claims = JSON.parse(new TextDecoder().decode(b64url(parts[1])));
  if (header.alg !== "RS256" || !header.kid) return null;

  const key = (await jwks()).find((k) => k.kid === header.kid);
  if (!key) return null;
  const cryptoKey = await crypto.subtle.importKey(
    "jwk",
    { kty: key.kty, n: key.n, e: key.e, alg: "RS256", ext: true },
    { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" },
    false,
    ["verify"],
  );
  const ok = await crypto.subtle.verify(
    "RSASSA-PKCS1-v1_5",
    cryptoKey,
    b64url(parts[2]),
    new TextEncoder().encode(`${parts[0]}.${parts[1]}`),
  );
  if (!ok) return null;

  const now = Math.floor(Date.now() / 1000);
  if (claims.iss !== ISSUER) return null;
  if (typeof claims.exp !== "number" || claims.exp < now - 30) return null;
  if (typeof claims.nbf === "number" && claims.nbf > now + 30) return null;
  if (claims.ref !== REF) return null;
  if (!EVENTS.has(claims.event_name)) return null;
  return claims;
}

// FLAVOR_ENV_EXAMPLE -> example. Names are lower case on the wire and
// in /opt/geekgallery/<name>; the secret's spelling is the upper-case form.
function flavorNames(env) {
  return Object.keys(env)
    .filter((k) => k.startsWith(FLAVOR_PREFIX) && env[k])
    .map((k) => k.slice(FLAVOR_PREFIX.length).toLowerCase())
    .sort();
}

function secretFor(name) {
  // Only what a directory name and an env var name both accept.
  if (!/^[a-z0-9][a-z0-9_-]{0,40}$/.test(name)) return null;
  return FLAVOR_PREFIX + name.toUpperCase().replace(/-/g, "_");
}

const forbidden = () => new Response(null, { status: 403 });

function text(body, claims) {
  return new Response(body, {
    status: body ? 200 : 503,
    headers: {
      "content-type": "text/plain; charset=utf-8",
      "cache-control": "no-store",
      "x-served-to": `${claims.repository}@${claims.sha || "?"}`,
    },
  });
}

export default {
  async fetch(request, env) {
    if (request.method !== "POST") return forbidden();
    const auth = request.headers.get("authorization") || "";
    const token = auth.startsWith("Bearer ") ? auth.slice(7).trim() : "";
    if (!token) return forbidden();

    let claims;
    try {
      claims = await verify(token);
    } catch (e) {
      return forbidden();
    }
    if (!claims) return forbidden();

    const path = new URL(request.url).pathname;
    if (claims.aud !== AUDIENCE) return forbidden();
    if (claims.repository !== env.REPOSITORY) return forbidden();

    if (path === "/flavors") {
      return new Response(JSON.stringify(flavorNames(env)), {
        headers: { "content-type": "application/json", "cache-control": "no-store" },
      });
    }
    const m = path.match(/^\/flavor\/([^/]+)$/);
    if (m) {
      const secret = secretFor(m[1]);
      if (!secret) return forbidden();
      return text(env[secret] || "", claims);
    }
    return forbidden();
  },
};
