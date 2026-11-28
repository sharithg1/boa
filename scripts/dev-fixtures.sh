#!/usr/bin/env bash
# Loads Boa's development / live-test fixtures (scripts/dev-fixtures.sql) into
# the Postgres server the PG* environment variables point at. Idempotent.
#
#   scripts/dev-fixtures.sh            # PGHOST=localhost PGPORT=5432 PGUSER=boa PGPASSWORD=boa
#   PGPORT=5433 scripts/dev-fixtures.sh
#
# Uses a local psql when there is one (CI); otherwise runs psql inside the
# Docker container named by BOA_PG_CONTAINER (default boa-postgres, see
# AGENTS.md), connecting to that container's own server.
#
# Needs a role that can create roles and databases (locally boa/boa, superuser).
# Leaves public.customers / public.orders in boa_dev alone (creates them only
# when missing). Rebuilds the boa_fx* schemas in boa_dev and the fixtures in
# boa_dev2; see the header of dev-fixtures.sql for the full list.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

export PGHOST="${PGHOST:-localhost}"
export PGPORT="${PGPORT:-5432}"
export PGUSER="${PGUSER:-boa}"
export PGPASSWORD="${PGPASSWORD:-boa}"
# The fixtures always go into boa_dev / boa_dev2, whatever PGDATABASE says.
db=boa_dev

container="${BOA_PG_CONTAINER:-boa-postgres}"
if [[ -n "${PSQL:-}" ]]; then
  psql_bin=("$PSQL")
elif command -v psql >/dev/null 2>&1; then
  psql_bin=(psql)
elif [[ -x /opt/homebrew/opt/postgresql@17/bin/psql ]]; then
  psql_bin=(/opt/homebrew/opt/postgresql@17/bin/psql)
elif [[ -x /usr/local/opt/postgresql@17/bin/psql ]]; then
  psql_bin=(/usr/local/opt/postgresql@17/bin/psql)
elif command -v docker >/dev/null 2>&1 &&
  [[ "$(docker inspect -f '{{.State.Running}}' "$container" 2>/dev/null)" == true ]]; then
  # psql inside the container talks to the container's server on its own port.
  psql_bin=(docker exec -i -e PGUSER -e PGPASSWORD "$container" psql -h localhost -p 5432)
else
  echo "dev-fixtures: psql not found and container $container is not running" >&2
  echo "  (start it as in AGENTS.md, or set PSQL=/path/to/psql)" >&2
  exit 1
fi

# boa_dev itself may be missing on a fresh server (CI): create it from the
# maintenance database first.
if ! exists="$("${psql_bin[@]}" -X -At -d postgres -c "select 1 from pg_database where datname = '$db'")"; then
  echo "dev-fixtures: cannot connect to $PGHOST:$PGPORT as $PGUSER (is Postgres running?)" >&2
  exit 1
fi
if [[ "$exists" != 1 ]]; then
  echo "dev-fixtures: creating database $db"
  "${psql_bin[@]}" -X -v ON_ERROR_STOP=1 -d postgres -c "create database $db"
fi

# Fed on stdin so the docker exec form can read it too.
"${psql_bin[@]}" -X -v ON_ERROR_STOP=1 -d "$db" < "$here/dev-fixtures.sql"

echo "dev-fixtures: loaded into $PGHOST:$PGPORT ($db, boa_dev2; role boa_ro)"
