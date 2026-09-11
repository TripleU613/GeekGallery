# deploy-env broker

A Cloudflare Worker that hands each flavor's deploy environment to GitHub
Actions in exchange for the job's own OIDC token. No GitHub secret is
involved: the job proves it is a push to `main` of the repository named in
`wrangler.toml`, signed by GitHub, and gets back the `KEY=VALUE` file for the
flavor it asks for.

One Worker secret per flavor, `FLAVOR_ENV_<NAME>`, holding the whole file
(see `FLAVORS.md` at the repository root for every key). The workflow finds
the broker through the `BROKER_URL` repository variable, asks `/flavors` for
the list, then `/flavor/<name>` for each, and deploys them all. So:

- **Add a site:** `wrangler secret put FLAVOR_ENV_NEWSITE < newsite.env`, then
  push (or run the workflow from the Actions tab). Nothing in the repository
  changes.
- **Rotate a value:** re-run `wrangler secret put` for that flavor with the
  edited file. The next deploy picks it up.
- **Remove a site:** `wrangler secret delete FLAVOR_ENV_OLDSITE`; the next
  deploy simply no longer touches it (take its container down by hand).

The Worker answers `403` with an empty body to anything that is not exactly
right: wrong method, no token, bad signature, wrong audience, wrong repo,
wrong branch, expired, unknown path.
