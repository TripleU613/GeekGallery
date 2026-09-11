# Contributing

Thanks for looking. geekgallery is small enough that one person can hold the
whole thing in their head, and the rules below exist to keep it that way.

## Before you start

- **Read `CLAUDE.md` first.** It is not documentation; it is the list of ways
  this codebase has already been broken, and most of them are not obvious
  (hydration mismatches, Tailwind's scanner, D1's lack of transactions).
  Every entry was paid for.
- **Open an issue before a large change.** A new page, a schema change, a new
  external dependency or a change to the deploy path is worth a conversation
  first. A bug fix, a copy fix or a new `THEME_*`/`SITE_*` knob is not.
- **Nothing site-specific goes in the code.** The whole point of the project
  is that a deployment is configured, not forked. If your change needs a
  name, a colour, a noun or a URL, it belongs in `src/flavor.rs` as a new key
  with a default, documented in `FLAVORS.md`.

## Setting up

```bash
cp .env.example .env            # point at a D1 database and (optionally) an R2 bucket
npm install                     # Tailwind only
cargo install cargo-leptos --locked
cargo leptos watch              # http://127.0.0.1:3100
```

There is no local database: dev talks to a real D1. Make yourself one
(`scripts/cloudflare-provision.sh`, or the Cloudflare dashboard) and run
`scripts/d1-migrate.sh <name>`. Uploads go to `./uploads` on disk when no R2
bucket is configured, which is fine for development.

## The gate

Run this before you push. CI runs exactly the same thing, and
`git config core.hooksPath .githooks` runs it on every `git push` for you.

```bash
cargo fmt --all -- --check
cargo clippy --no-default-features --features ssr --all-targets -- -D warnings
cargo clippy --no-default-features --features hydrate --target wasm32-unknown-unknown -- -D warnings
cargo test  --no-default-features --features ssr
scripts/audit-secrets.sh
```

Both feature sets, always: `ssr` and `hydrate` compile different code from the
same files, and one passing says nothing about the other.

## Style

- Match the surrounding code. Comments explain *why*, and the codebase leans
  on them heavily; a comment that restates the line below it is noise.
- Tailwind classes are whole literal strings, never assembled from fragments
  (the scanner reads `.rs` files as text). Never layer a utility on a
  primitive that already sets that property; write the whole class string per
  state.
- Colours are the `--c-*` variables, never a hex literal.
- Anything that differs between server and client must not seed initial state.
  Read the hydration section of `CLAUDE.md` twice.
- A new Axum route needs `rel="external"` on every anchor that points at it.
  `tests/router_links.rs` will tell you if you forget.

## Commits and pull requests

- One change per pull request, described in the message the way you would
  explain it to someone reading `git log` in a year: what was wrong, what this
  does, why this way.
- If you hit a trap that is not in `CLAUDE.md`, add it. Two lines, what broke
  and what to do instead. That file is the most valuable one in the repository.
- Never commit a credential, a `.env`, or a flavor file. `scripts/audit-secrets.sh`
  scans history, tracked files and build output, and CI runs it.

## Reporting a security problem

See `SECURITY.md`. Please do not open a public issue for anything exploitable.

## Code of conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md).
