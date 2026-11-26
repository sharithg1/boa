#!/usr/bin/env bash
# Loads Boa's development / live-test fixtures (scripts/dev-fixtures.sql) into
# the Postgres server the PG* environment variables point at. Idempotent.
#
#   scripts/dev-fixtures.sh            # PGHOST=localhost PGPORT=5432 PGUSER=boa PGPASSWORD=boa
#   PGPORT=5433 scripts/dev-fixtures.sh
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

psql_bin="${PSQL:-}"
if [[ -z "$psql_bin" ]]; then
  if command -v psql >/dev/null 2>&1; then
    psql_bin=psql
  elif [[ -x /opt/homebrew/opt/postgresql@17/bin/psql ]]; then
    psql_bin=/opt/homebrew/opt/postgresql@17/bin/psql
  elif [[ -x /usr/local/opt/postgresql@17/bin/psql ]]; then
    psql_bin=/usr/local/opt/postgresql@17/bin/psql
  else
    echo "dev-fixtures: psql not found (set PSQL=/path/to/psql)" >&2
    exit 1
  fi
fi

# boa_dev itself may be missing on a fresh server (CI): create it from the
# maintenance database first.
if ! exists="$("$psql_bin" -X -At -d postgres -c "select 1 from pg_database where datname = '$db'")"; then
  echo "dev-fixtures: cannot connect to $PGHOST:$PGPORT as $PGUSER (is Postgres running?)" >&2
  exit 1
fi
if [[ "$exists" != 1 ]]; then
  echo "dev-fixtures: creating database $db"
  "$psql_bin" -X -v ON_ERROR_STOP=1 -d postgres -c "create database $db"
fi

"$psql_bin" -X -v ON_ERROR_STOP=1 -d "$db" -f "$here/dev-fixtures.sql"

echo "dev-fixtures: loaded into $PGHOST:$PGPORT ($db, boa_dev2; role boa_ro)"
