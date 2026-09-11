#!/usr/bin/env bash
# Runs on the deploy host, fed to `bash -s` over SSH stdin by the CI deploy step
# with these already exported into this shell:
#
#   GALLERY_FLAVOR   the flavor's name, which is also its directory under /opt
#   GALLERY_IMAGE    the image to run, by digest
#   GALLERY_ENV      the flavor's whole KEY=VALUE configuration
#   TUNNEL_TOKEN     cloudflared's, lifted out of GALLERY_ENV by CI
#   LEGACY_COMPOSE   optional: a compose file of the stack this flavor replaces
#
# Never executed locally, and never passed as an SSH argument -- on a command line
# these values sit in the host's process list for the length of the deploy.
#
# It exists as a file rather than a heredoc in the workflow so it can be read,
# shellchecked and changed without touching YAML quoting rules.
set -euo pipefail

: "${GALLERY_FLAVOR:?}" "${GALLERY_IMAGE:?}" "${GALLERY_ENV:?}" "${TUNNEL_TOKEN:?}"

DIR="/opt/geekgallery/$GALLERY_FLAVOR"
# An explicit project name: compose would otherwise derive it from the
# directory, and two flavors named alike in different parents would collide.
COMPOSE="docker compose -p gg-$GALLERY_FLAVOR -f $DIR/docker-compose.yml"
SERVICE=gallery

# The stack this flavor takes over from, if any (a single-site deploy of an
# earlier build, living in its own directory under /opt). Taken down BEFORE
# the new one comes up, because both carry the same tunnel token, and two
# cloudflared processes on one token is Cloudflare load-balancing between an
# old origin and a new one. The file is moved aside rather than deleted so
# this only ever happens once and the old stack stays restorable by hand.
retired_legacy=""
if [ -n "${LEGACY_COMPOSE:-}" ]; then
  if [ -f "$LEGACY_COMPOSE" ]; then
    echo "deploy: retiring the legacy stack at $LEGACY_COMPOSE"
    docker compose -f "$LEGACY_COMPOSE" down --remove-orphans || true
    mv "$LEGACY_COMPOSE" "$LEGACY_COMPOSE.retired"
    retired_legacy="$LEGACY_COMPOSE"
  fi
  # `compose down` above is not enough on its own, and the first cutover
  # proved it: the old file says `image: ${SOME_IMAGE}`, that variable is not
  # in this shell, and compose refuses to parse a service with no image --
  # so `down` failed, the file was moved aside, and the old container kept
  # serving beside the new one. The containers carry the old project's
  # label (compose names a project after its directory), so remove them by
  # that, whatever the file says.
  legacy_project=$(basename "$(dirname "$LEGACY_COMPOSE")")
  ids=$(docker ps -aq --filter "label=com.docker.compose.project=$legacy_project")
  if [ -n "$ids" ]; then
    echo "deploy: removing the legacy '$legacy_project' container(s) still present"
    # shellcheck disable=SC2086
    docker rm -f $ids >/dev/null
  fi
fi

# What is running right now, so there is something to go back to. Read before the
# pull, because the pull is what makes the new image available and the `up` is what
# makes it live.
#
# `.Config.Image` on the container rather than the compose file's ${GALLERY_IMAGE}:
# the file has already been overwritten by the scp above with the new one.
prev_image="$($COMPOSE ps -q "$SERVICE" 2>/dev/null \
  | xargs -r docker inspect --format '{{.Config.Image}}' 2>/dev/null || true)"

$COMPOSE pull
# --remove-orphans so a renamed service never leaves its predecessor running
# beside it -- two cloudflared processes on one tunnel token is the "two origins
# on one tunnel" failure that once had Cloudflare load-balancing between an old
# stack and a new one.
$COMPOSE up -d --remove-orphans

# Wait for the container to say it is serving, and put the old one back if it never
# does.
#
# Without this a broken image replaced a working container and the site stayed down
# until someone pushed again -- the deploy reported success either way, because
# `docker compose up` succeeds when the container *starts*, which is a much weaker
# claim than "the site answers".
#
# The healthcheck this reads is a TCP connect to the app's port and nothing else
# (see the Dockerfile).
deadline=$((SECONDS + 180))
status=starting
while [ "$SECONDS" -lt "$deadline" ]; do
  status="$($COMPOSE ps -q "$SERVICE" \
    | xargs -r docker inspect --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}none{{end}}' \
    2>/dev/null || echo missing)"
  case "$status" in
    healthy) echo "deploy: $GALLERY_FLAVOR healthy after $SECONDS s"; break ;;
    # No healthcheck in the image at all: nothing to wait for, and waiting three
    # minutes to conclude that would be its own kind of wrong.
    none) echo "deploy: image declares no healthcheck; not gating on it"; break ;;
    unhealthy)
      # Keep looking: a cold start can report unhealthy for its first interval
      # before the app has bound its port, and start-period only suppresses the
      # *failure count*, not the status string.
      ;;
  esac
  sleep 5
done

if [ "$status" != healthy ] && [ "$status" != none ]; then
  echo "deploy: $GALLERY_FLAVOR still '$status' after 180s -- rolling back" >&2
  # The last 60 lines of a container that never came up are the only diagnosis
  # anyone gets, and they are gone the moment it is replaced.
  $COMPOSE logs --tail 60 "$SERVICE" >&2 || true
  if [ -n "$prev_image" ]; then
    echo "deploy: restoring $prev_image" >&2
    GALLERY_IMAGE="$prev_image" $COMPOSE up -d --force-recreate "$SERVICE"
  elif [ -n "$retired_legacy" ]; then
    # The first deploy over a per-repo stack: there is no previous image of
    # this flavor to go back to, but the stack it just retired is one `mv`
    # away. Its containers still hold their environment, so `up` needs none.
    echo "deploy: bringing the legacy stack at $retired_legacy back" >&2
    $COMPOSE down --remove-orphans || true
    mv "$retired_legacy.retired" "$retired_legacy"
    docker compose -f "$retired_legacy" up -d --no-recreate || true
  else
    echo "deploy: nothing was running before this, so there is nothing to restore" >&2
  fi
  exit 1
fi
