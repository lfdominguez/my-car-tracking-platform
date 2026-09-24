#!/usr/bin/env bash
# Back up the database and the uploads volume of the docker compose stack.
#
#   scripts/backup.sh [BACKUP_DIR]      (default: ./backups)
#
# Env: KEEP (default 14) = number of backups kept; COMPOSE (default
# "docker compose") = the compose command. Run it from cron, e.g.
#   15 3 * * *  cd /srv/car-tracking && scripts/backup.sh /srv/backups
# See docs/backup.md for restoring.
set -euo pipefail

dir="${1:-./backups}"
keep="${KEEP:-14}"
compose="${COMPOSE:-docker compose}"
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
out="$dir/$stamp"
mkdir -p "$out"

# Custom format: compressed, and pg_restore can restore it in parallel.
$compose exec -T db sh -c \
  'pg_dump -U "${POSTGRES_USER:-ctp}" -d "${POSTGRES_DB:-car_tracking}" -Fc' \
  > "$out/db.dump"

# Car photos and other uploads live outside the database.
$compose exec -T app tar -C /app/data/uploads -czf - . > "$out/uploads.tar.gz"

# A dump that pg_restore cannot list is not a backup.
$compose exec -T db pg_restore --list < "$out/db.dump" > /dev/null

echo "backup written to $out ($(du -sh "$out" | cut -f1))"

# Rotation: keep the newest $keep backups.
ls -1d "$dir"/*/ 2>/dev/null | sort | head -n "-$keep" | xargs -r rm -rf
