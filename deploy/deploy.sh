#!/bin/sh
# Deploy the latest `main` if it hasn't been deployed yet. Run every minute
# by sisyphus-deploy.timer; `deploy.sh --force` redeploys regardless.
#
# The last successfully deployed commit is kept in $STATE, so a failed
# build is retried on the next run instead of being skipped.
set -eu

APP=/opt/sisyphus/app
ENV_FILE=/opt/sisyphus/sisyphus.env
STATE=/opt/sisyphus/deployed-commit

# One deploy at a time.
exec 9>/run/sisyphus-deploy.lock
flock -n 9 || exit 0

cd "$APP"
git fetch --quiet origin main
target=$(git rev-parse origin/main)
if [ "${1:-}" != "--force" ] && [ -f "$STATE" ] && [ "$(cat "$STATE")" = "$target" ]; then
    exit 0
fi

git reset --quiet --hard "$target"
echo "deploying $(git log -1 --format='%h %s' "$target")"
docker compose -f deploy/compose.yaml --env-file "$ENV_FILE" up -d --build --remove-orphans
echo "$target" > "$STATE"
docker image prune -f > /dev/null
echo "deployed $(git rev-parse --short "$target")"
