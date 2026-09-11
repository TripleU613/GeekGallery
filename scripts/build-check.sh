#!/usr/bin/env bash
# Build the production image before pushing, on a host that actually has Docker.
#
# Why this exists: this dev machine has no container runtime and no sudo to install
# one, so `RUN` steps in the Dockerfile were only ever exercised by CI. Three
# consecutive CI failures came from that blind spot -- a base digest that did not
# resolve, then `awscli` having no installation candidate on the Playwright base --
# each costing ~12 minutes to learn something a build says in two.
#
# bulky-server has Docker and is reachable over Tailscale SSH, so the context is
# piped to it and built there. Nothing is pushed or tagged into the deploy.
#
#   scripts/build-check.sh              # build only
#   scripts/build-check.sh --run        # build, then start it and check both
#                                       # processes actually come up
#   scripts/build-check.sh --audit      # build, then scan its layers for secrets
#
# One image (app + cloudflared). Not in the pre-push hook because a full release
# build of the Rust tree is minutes, not seconds; run it when the Dockerfile,
# supervisord.conf or docker-compose.yml changes.
set -euo pipefail

HOST="${BUILD_HOST:-root@bulky-server}"
TAG="geekgallery-buildcheck"
RUN=0
AUDIT=0
for arg in "$@"; do
  case "$arg" in
    --run) RUN=1 ;;
    --audit) AUDIT=1 ;;
    *) echo "build-check: unknown argument $arg" >&2; exit 2 ;;
  esac
done

cd "$(dirname "$0")/.."

echo "build-check: building $TAG on $HOST"
# The context goes over stdin as a tarball, so nothing has to be copied to the host
# first. Excludes mirror .dockerignore -- the daemon would otherwise be sent
# target/ (multi-GB) and .env, and the whole point of .dockerignore is that .env
# never reaches a build.
if tar -cz \
      --exclude=./target --exclude=./.git --exclude=./node_modules \
      --exclude=./.env --exclude='./.env.*' --exclude=./.playwright-mcp \
      -C . . \
    | ssh "$HOST" "docker build --rm -t $TAG -f Dockerfile -" 2>&1 | tail -30
then
  echo "build-check: build OK"
else
  echo "build-check: build FAILED" >&2
  exit 1
fi

if [ "$AUDIT" = 1 ]; then
  # The image is on the remote host, so the scan runs there -- with the repo's own
  # script and .env copied over a pipe, used, and deleted. .env never lands on
  # disk anywhere else: it goes to a private tmp dir that is removed in a trap,
  # even on failure.
  echo "build-check: scanning $TAG's layers for secrets"
  tar -cz scripts/audit-secrets.sh .env \
    | ssh "$HOST" "set -e
        d=\$(mktemp -d); trap 'rm -rf \"\$d\"' EXIT
        tar -xz -C \"\$d\"
        cd \"\$d\" && git init -q . && git add -A 2>/dev/null || true
        ./scripts/audit-secrets.sh --image $TAG" \
    || { echo "build-check: SECRET FOUND IN IMAGE" >&2; exit 1; }
fi

if [ "$RUN" = 1 ]; then
  # A real start, because four supervised processes in one container is exactly the
  # kind of thing that builds fine and then does not run. No tunnel token, so
  # cloudflared fails its own start and supervisor reports it -- everything else
  # must still come up.
  echo "build-check: starting $TAG (no tunnel, no credentials)"
  ssh "$HOST" "
    docker rm -f $TAG-run >/dev/null 2>&1 || true
    docker run -d --name $TAG-run \
      --read-only --tmpfs /tmp:exec,size=700m --shm-size 256m \
      -m 2g --cpus 1.5 \
      -e TUNNEL_TOKEN= \
      -e GEMINI_COOKIES= \
      -p 127.0.0.1:3199:3100 \
      $TAG >/dev/null
    sleep 25
    echo '--- supervisor ---'
    docker exec $TAG-run supervisorctl -c /etc/supervisor/supervisord.conf status || true
    echo '--- listening ---'
    docker exec $TAG-run bash -c 'ss -ltn 2>/dev/null || netstat -ltn' || true
    echo '--- app answers ---'
    curl -s -o /dev/null -w 'GET / -> %{http_code}\n' http://127.0.0.1:3199/ || true
    echo '--- writable paths (read-only rootfs) ---'
    docker exec $TAG-run bash -c 'touch /app/x 2>&1 | head -1; touch /tmp/x && echo \"/tmp writable: yes\"' || true
    docker logs --tail 25 $TAG-run
    docker rm -f $TAG-run >/dev/null
  "
fi

echo "build-check: cleaning up on $HOST"
# Only the image this script made, and only dangling layers -- never a blanket
# prune, which would evict the image production is running from.
ssh "$HOST" "
  docker image rm -f $TAG >/dev/null 2>&1 || true
  docker image prune -f >/dev/null 2>&1 || true
" || true

echo "build-check: done"
