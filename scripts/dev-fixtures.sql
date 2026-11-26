-- Boa development / live-test fixtures. Idempotent: run it as often as you like.
--
-- Run through scripts/dev-fixtures.sh (it sets ON_ERROR_STOP and the
-- connection), or directly:
--   psql -X -v ON_ERROR_STOP=1 -d boa_dev -f scripts/dev-fixtures.sql
-- as a role that can create roles and databases (locally: boa/boa, superuser).
--
-- What it owns (dropped and rebuilt on every run, so don't keep work there):
--   database boa_dev:  schemas boa_fx, boa_fx_many, "boa_fx_ünïcødé", boa_fx_empty
--   database boa_dev2: created if missing; schema boa_fx2 and its public.products / public.stock
--   role boa_ro (password boa_ro): read-only (SELECT on the fixture schemas,
--                       default_transaction_read_only = on)
--
-- public.customers / public.orders in boa_dev are never dropped or altered.
-- They are created and seeded only when missing (a fresh machine or CI), with
-- the same shape as the hand-made originals (5,000 customers, 100,000 orders).

\set QUIET on
SET client_min_messages = warning;

-- ---------------------------------------------------------------------------
-- Roles and the second database (cluster-wide; not transactional).
-- ---------------------------------------------------------------------------

DO $$
BEGIN
    IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'boa_ro') THEN
        CREATE ROLE boa_ro LOGIN PASSWORD 'boa_ro';
    END IF;
END
$$;
ALTER ROLE boa_ro LOGIN PASSWORD 'boa_ro' NOSUPERUSER NOCREATEDB NOCREATEROLE;
-- Server-side read-only on top of the missing write grants: even a table the
-- role could write to (none today) refuses writes unless a session opts out.
ALTER ROLE boa_ro SET default_transaction_read_only = on;

SELECT 'CREATE DATABASE boa_dev2'
WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = 'boa_dev2') \gexec

-- ---------------------------------------------------------------------------
-- boa_dev
-- ---------------------------------------------------------------------------
\connect boa_dev
\set QUIET on
SET client_min_messages = warning;

BEGIN;

-- public.customers / public.orders: only when missing. -----------------------
DO $$
BEGIN
    IF to_regclass('public.customers') IS NULL THEN
        CREATE TABLE public.customers (
            id serial PRIMARY KEY,
            name text NOT NULL,
            email text UNIQUE,
            country text,
            created_at timestamptz DEFAULT now()
        );
        INSERT INTO public.customers (name, email, country)
        SELECT 'Customer ' || g, 'c' || g || '@example.com',
               (ARRAY['US', 'DE', 'LK', 'JP', 'BR'])[g % 5 + 1]
        FROM generate_series(1, 5000) g;
    END IF;
    IF to_regclass('public.orders') IS NULL THEN
        CREATE TABLE public.orders (
            id bigserial PRIMARY KEY,
            customer_id integer NOT NULL REFERENCES public.customers (id),
            total numeric(12, 2),
            status text,
            notes text,
            payload jsonb,
            placed_at timestamptz
        );
        CREATE INDEX orders_customer_idx ON public.orders (customer_id);
        INSERT INTO public.orders (customer_id, total, status, notes, payload, placed_at)
        SELECT g % 5000 + 1,
               round((((g * 7919) % 99900) / 100.0 + 1)::numeric, 2),
               (ARRAY['new', 'paid', 'shipped', 'cancelled'])[g % 4 + 1],
               'note ' || g,
               jsonb_build_object('n', g, 'tags', jsonb_build_array('a', 'b')),
               now() - make_interval(mins => g)
        FROM generate_series(1, 100000) g;
    END IF;
END
$$;

-- Fixture schemas: rebuilt from scratch. -------------------------------------
DROP SCHEMA IF EXISTS boa_fx_many CASCADE;
DROP SCHEMA IF EXISTS "boa_fx_ünïcødé" CASCADE;
DROP SCHEMA IF EXISTS boa_fx_empty CASCADE;
DROP SCHEMA IF EXISTS boa_fx CASCADE;

CREATE SCHEMA boa_fx;
COMMENT ON SCHEMA boa_fx IS 'Boa dev fixtures: types, constraints, views, slow function';
CREATE SCHEMA boa_fx_many;
COMMENT ON SCHEMA boa_fx_many IS 'Boa dev fixtures: 50 tables for the explorer';
CREATE SCHEMA "boa_fx_ünïcødé";
COMMENT ON SCHEMA "boa_fx_ünïcødé" IS 'Boa dev fixtures: non-ASCII and quoted identifiers';
CREATE SCHEMA boa_fx_empty;
COMMENT ON SCHEMA boa_fx_empty IS 'Boa dev fixtures: a schema with nothing in it';

SET LOCAL search_path = boa_fx;

-- Types. --------------------------------------------------------------------
CREATE TYPE boa_fx.mood AS ENUM ('sad', 'ok', 'happy');
CREATE DOMAIN boa_fx.percent AS numeric(5, 2) CHECK (VALUE BETWEEN 0 AND 100);

-- Every column type the grid / value formatting cares about. 1,000 rows,
-- deterministic values, NULLs sprinkled in, a few long / multi-line / big JSON values.
CREATE TABLE boa_fx.all_types (
    id integer PRIMARY KEY,
    small_int smallint,
    big_int bigint,
    num numeric,
    money_like numeric(12, 2),
    real_val real,
    double_val double precision,
    flag boolean,
    label varchar(40),
    code char(3),
    body text,
    doc jsonb,
    doc_json json,
    tags text[],
    scores integer[],
    matrix integer[][],
    raw bytea,
    uid uuid,
    born date,
    at_time time,
    at_ts timestamp,
    at_tstz timestamptz,
    span interval,
    addr inet,
    net cidr,
    feeling boa_fx.mood,
    pct boa_fx.percent,
    xml_val xml,
    pt point,
    range_val int4range
);
COMMENT ON TABLE boa_fx.all_types IS 'One column per interesting type';
COMMENT ON COLUMN boa_fx.all_types.doc IS 'jsonb, nested; row 7 is large';

INSERT INTO boa_fx.all_types
SELECT g,
       (g % 32767)::smallint,
       g::bigint * 1000000007,
       CASE WHEN g % 10 = 0 THEN NULL ELSE (g * 3.14159265358979)::numeric END,
       round((g * 12.34)::numeric, 2),
       (g / 7.0)::real,
       g / 3.0,
       CASE g % 3 WHEN 0 THEN NULL WHEN 1 THEN true ELSE false END,
       'label ' || g,
       lpad((g % 1000)::text, 3, '0'),
       CASE
           WHEN g % 50 = 0 THEN repeat('long text ' || g || ' ', 40)      -- > 200 chars
           WHEN g % 50 = 1 THEN 'line one' || E'\n' || 'line two' || E'\n' || 'line three'
           WHEN g % 50 = 2 THEN ''
           WHEN g % 10 = 3 THEN NULL
           ELSE 'text ' || g
       END,
       CASE
           WHEN g = 7 THEN (SELECT jsonb_agg(jsonb_build_object('i', i, 'name', 'item ' || i,
                                   'nested', jsonb_build_object('a', i, 'b', ARRAY[i, i + 1])))
                            FROM generate_series(1, 200) i)
           WHEN g % 10 = 4 THEN NULL
           ELSE jsonb_build_object('n', g, 'even', g % 2 = 0, 'tags', jsonb_build_array('x', g),
                                   'obj', jsonb_build_object('k', 'v' || g, 'nil', NULL))
       END,
       json_build_object('n', g, 'note', 'keeps   spacing and key order'),
       CASE WHEN g % 10 = 5 THEN NULL ELSE ARRAY['a' || g, 'b', 'with space', 'quote"d'] END,
       ARRAY[g, g * 2, g * 3],
       ARRAY[ARRAY[g, g + 1], ARRAY[g + 2, g + 3]],
       CASE WHEN g % 10 = 6 THEN NULL ELSE decode(md5(g::text), 'hex') END,
       md5('uuid' || g)::uuid,
       date '2020-01-01' + g,
       time '00:00' + make_interval(secs => g * 37),
       timestamp '2024-01-01 00:00' + make_interval(mins => g * 61),
       timestamptz '2024-01-01 00:00+00' + make_interval(hours => g),
       make_interval(days => g % 40, hours => g % 24, secs => g % 60),
       ('10.0.' || (g / 256) % 256 || '.' || g % 256)::inet,
       ('10.' || g % 256 || '.0.0/16')::cidr,
       (ARRAY['sad', 'ok', 'happy'])[g % 3 + 1]::boa_fx.mood,
       (g % 101)::numeric,
       ('<row id="' || g || '"/>')::xml,
       point(g, -g),
       int4range(g, g + 10)
FROM generate_series(1, 1000) g;

-- Defaults, identity, generated columns. --------------------------------------
CREATE TABLE boa_fx.products (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    sku text NOT NULL UNIQUE,
    name text NOT NULL,
    price numeric(10, 2) NOT NULL DEFAULT 0,
    qty integer NOT NULL DEFAULT 1,
    total numeric(12, 2) GENERATED ALWAYS AS (price * qty) STORED,
    active boolean NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT now(),
    ref uuid NOT NULL DEFAULT gen_random_uuid(),
    CONSTRAINT products_price_nonneg CHECK (price >= 0),
    CONSTRAINT products_qty_range CHECK (qty BETWEEN 0 AND 10000)
);
COMMENT ON TABLE boa_fx.products IS 'Identity PK, defaults, a generated column, checks';
INSERT INTO boa_fx.products (sku, name, price, qty, created_at)
SELECT 'SKU-' || lpad(g::text, 4, '0'), 'Product ' || g, (g % 500) + 0.99, g % 50,
       timestamptz '2025-01-01 00:00+00' + make_interval(days => g)
FROM generate_series(1, 300) g;

CREATE TABLE boa_fx.counters (
    id serial PRIMARY KEY,
    name text NOT NULL DEFAULT 'unnamed',
    hits bigint NOT NULL DEFAULT 0,
    updated_at timestamp DEFAULT (now() AT TIME ZONE 'utc')
);
INSERT INTO boa_fx.counters (name, hits) SELECT 'counter ' || g, g * 10 FROM generate_series(1, 20) g;

-- Check constraints (column, table, multi-column, NOT VALID). -----------------
CREATE TABLE boa_fx.bookings (
    id integer PRIMARY KEY,
    guest text NOT NULL CHECK (length(guest) > 0),
    starts_on date NOT NULL,
    ends_on date NOT NULL,
    guests smallint NOT NULL DEFAULT 1 CONSTRAINT bookings_guests_positive CHECK (guests > 0),
    status text NOT NULL DEFAULT 'pending',
    CONSTRAINT bookings_dates_ordered CHECK (ends_on > starts_on),
    CONSTRAINT bookings_status_known CHECK (status IN ('pending', 'confirmed', 'cancelled'))
);
INSERT INTO boa_fx.bookings
SELECT g, 'Guest ' || g, date '2026-01-01' + g, date '2026-01-01' + g + 1 + g % 7, 1 + g % 4,
       (ARRAY['pending', 'confirmed', 'cancelled'])[g % 3 + 1]
FROM generate_series(1, 200) g;
ALTER TABLE boa_fx.bookings ADD CONSTRAINT bookings_short_stay CHECK (ends_on - starts_on <= 30) NOT VALID;

-- Multi-column primary and foreign keys, self reference, ON DELETE actions. ---
CREATE TABLE boa_fx.regions (
    country_code char(2) NOT NULL,
    region_code text NOT NULL,
    name text NOT NULL,
    PRIMARY KEY (country_code, region_code)
);
INSERT INTO boa_fx.regions VALUES
    ('DE', 'BE', 'Berlin'), ('DE', 'BY', 'Bayern'), ('LK', 'WP', 'Western'),
    ('LK', 'CP', 'Central'), ('JP', '13', 'Tōkyō'), ('US', 'CA', 'California');

CREATE TABLE boa_fx.stores (
    id integer PRIMARY KEY,
    country_code char(2) NOT NULL,
    region_code text NOT NULL,
    name text NOT NULL,
    CONSTRAINT stores_region_fkey FOREIGN KEY (country_code, region_code)
        REFERENCES boa_fx.regions (country_code, region_code) ON UPDATE CASCADE
);
INSERT INTO boa_fx.stores
SELECT g, r.country_code, r.region_code, 'Store ' || g
FROM generate_series(1, 30) g
JOIN LATERAL (SELECT * FROM boa_fx.regions ORDER BY country_code, region_code
              OFFSET (g % 6) LIMIT 1) r ON true;

CREATE TABLE boa_fx.employees (
    id integer PRIMARY KEY,
    store_id integer NOT NULL REFERENCES boa_fx.stores (id) ON DELETE CASCADE,
    manager_id integer REFERENCES boa_fx.employees (id) ON DELETE SET NULL,
    name text NOT NULL,
    email text,
    hired_on date NOT NULL DEFAULT current_date
);
CREATE UNIQUE INDEX employees_email_lower_key ON boa_fx.employees (lower(email)) WHERE email IS NOT NULL;
CREATE INDEX employees_store_idx ON boa_fx.employees (store_id) INCLUDE (name);
INSERT INTO boa_fx.employees (id, store_id, manager_id, name, email, hired_on)
SELECT g, g % 30 + 1, CASE WHEN g > 30 THEN g % 30 + 1 END, 'Employee ' || g,
       CASE WHEN g % 5 = 0 THEN NULL ELSE 'e' || g || '@example.com' END,
       date '2020-01-01' + g * 3
FROM generate_series(1, 150) g;

-- Composite FK referencing a composite PK (order lines -> order heads).
CREATE TABLE boa_fx.order_heads (
    store_id integer NOT NULL REFERENCES boa_fx.stores (id),
    order_no integer NOT NULL,
    placed_at timestamptz NOT NULL,
    PRIMARY KEY (store_id, order_no)
);
CREATE TABLE boa_fx.order_lines (
    store_id integer NOT NULL,
    order_no integer NOT NULL,
    line_no smallint NOT NULL,
    product_id bigint NOT NULL REFERENCES boa_fx.products (id) ON DELETE RESTRICT,
    qty integer NOT NULL CHECK (qty > 0),
    PRIMARY KEY (store_id, order_no, line_no),
    CONSTRAINT order_lines_head_fkey FOREIGN KEY (store_id, order_no)
        REFERENCES boa_fx.order_heads (store_id, order_no) ON DELETE CASCADE
);
INSERT INTO boa_fx.order_heads
SELECT s, o, timestamptz '2026-01-01 00:00+00' + make_interval(hours => s * 100 + o)
FROM generate_series(1, 30) s, generate_series(1, 20) o;
INSERT INTO boa_fx.order_lines
SELECT s, o, l, (s * 7 + o * 3 + l) % 300 + 1, 1 + (s + o + l) % 5
FROM generate_series(1, 30) s, generate_series(1, 20) o, generate_series(1, 3) l;

-- No primary key (grid editing must say why it's read-only). -------------------
CREATE TABLE boa_fx.event_log (
    at timestamptz NOT NULL,
    level text NOT NULL,
    message text,
    context jsonb
);
COMMENT ON TABLE boa_fx.event_log IS 'No primary key, duplicate rows on purpose';
INSERT INTO boa_fx.event_log
SELECT timestamptz '2026-06-01 00:00+00' + make_interval(secs => g / 2),  -- pairs share a timestamp
       (ARRAY['debug', 'info', 'warn', 'error'])[g % 4 + 1],
       'event ' || (g / 2),
       jsonb_build_object('g', g / 2)
FROM generate_series(1, 400) g;

-- A unique constraint but no PK.
CREATE TABLE boa_fx.settings_kv (
    key text NOT NULL UNIQUE,
    value text
);
INSERT INTO boa_fx.settings_kv VALUES ('theme', 'dark'), ('page_size', '500'), ('empty', NULL);

-- Partitioned table (relkind p) with two partitions.
CREATE TABLE boa_fx.measurements (
    sensor_id integer NOT NULL,
    taken_on date NOT NULL,
    reading double precision,
    PRIMARY KEY (sensor_id, taken_on)
) PARTITION BY RANGE (taken_on);
CREATE TABLE boa_fx.measurements_2025 PARTITION OF boa_fx.measurements
    FOR VALUES FROM ('2025-01-01') TO ('2026-01-01');
CREATE TABLE boa_fx.measurements_2026 PARTITION OF boa_fx.measurements
    FOR VALUES FROM ('2026-01-01') TO ('2027-01-01');
INSERT INTO boa_fx.measurements
SELECT s, date '2025-01-01' + d, sin(d / 10.0) * s
FROM generate_series(1, 5) s, generate_series(0, 700) d;

-- Views and a materialized view. ----------------------------------------------
CREATE VIEW boa_fx.active_products AS
    SELECT id, sku, name, price, qty, total FROM boa_fx.products WHERE active;
COMMENT ON VIEW boa_fx.active_products IS 'Simple (auto-updatable) view';

CREATE VIEW boa_fx.store_summary AS
    SELECT s.id, s.name, r.name AS region, count(e.id) AS employees
    FROM boa_fx.stores s
    JOIN boa_fx.regions r USING (country_code, region_code)
    LEFT JOIN boa_fx.employees e ON e.store_id = s.id
    GROUP BY s.id, s.name, r.name;

CREATE MATERIALIZED VIEW boa_fx.daily_lines AS
    SELECT date_trunc('day', h.placed_at)::date AS day, count(*) AS lines, sum(l.qty) AS qty
    FROM boa_fx.order_heads h JOIN boa_fx.order_lines l USING (store_id, order_no)
    GROUP BY 1
WITH DATA;
CREATE UNIQUE INDEX daily_lines_day_key ON boa_fx.daily_lines (day);
COMMENT ON MATERIALIZED VIEW boa_fx.daily_lines IS 'Materialized view with a unique index';

-- Functions. ------------------------------------------------------------------
-- For cancel tests: select boa_fx.slow(30);
CREATE FUNCTION boa_fx.slow(seconds double precision DEFAULT 10)
RETURNS double precision
LANGUAGE sql VOLATILE
AS $fn$ SELECT pg_sleep(seconds); SELECT seconds; $fn$;
COMMENT ON FUNCTION boa_fx.slow(double precision) IS 'Sleeps, then returns its argument (cancel tests)';

-- Rows that trickle out: select * from boa_fx.slow_rows(10, 0.5);
CREATE FUNCTION boa_fx.slow_rows(n integer, delay double precision DEFAULT 0.2)
RETURNS TABLE (i integer, at timestamptz)
LANGUAGE plpgsql VOLATILE
AS $fn$
BEGIN
    FOR k IN 1..n LOOP
        PERFORM pg_sleep(delay);
        i := k;
        at := clock_timestamp();
        RETURN NEXT;
    END LOOP;
END
$fn$;

CREATE FUNCTION boa_fx.fail(msg text DEFAULT 'boom')
RETURNS void
LANGUAGE plpgsql
AS $fn$ BEGIN RAISE EXCEPTION '%', msg USING DETAIL = 'raised by boa_fx.fail', HINT = 'it always does'; END $fn$;

-- boa_fx_many: 50 tables, chained FKs, varied widths. --------------------------
DO $$
DECLARE
    t text;
BEGIN
    FOR k IN 1..50 LOOP
        t := format('t%s', lpad(k::text, 2, '0'));
        EXECUTE format(
            'CREATE TABLE boa_fx_many.%I (id integer PRIMARY KEY, name text NOT NULL, %s created_at timestamptz DEFAULT now())',
            t,
            CASE WHEN k > 1 THEN format('parent_id integer REFERENCES boa_fx_many.%I (id), ',
                                        format('t%s', lpad((k - 1)::text, 2, '0')))
                 ELSE '' END
            || coalesce((SELECT string_agg(format('c%s text, ', c), '') FROM generate_series(1, k % 7) c), '')
        );
        EXECUTE format('COMMENT ON TABLE boa_fx_many.%I IS %L', t, 'explorer filler ' || k);
        EXECUTE format(
            'INSERT INTO boa_fx_many.%I (id, name%s) SELECT g, %L || g%s FROM generate_series(1, %s) g',
            t,
            CASE WHEN k > 1 THEN ', parent_id' ELSE '' END,
            t || ' row ',
            CASE WHEN k > 1 THEN format(', (g - 1) %% %s + 1', (k - 1) * 2) ELSE '' END,
            k * 2
        );
        IF k % 10 = 0 THEN
            EXECUTE format('CREATE VIEW boa_fx_many.%I AS SELECT id, name FROM boa_fx_many.%I', 'v_' || t, t);
        END IF;
    END LOOP;
END
$$;

-- Non-ASCII and quoted identifiers. -------------------------------------------
CREATE TABLE "boa_fx_ünïcødé"."Kunden" (
    "Nr" integer PRIMARY KEY,
    "Größe" numeric(5, 1),
    "名前" text NOT NULL,
    "Straße & Hausnummer" text,
    "emoji 🐘" text,
    "Mixed Case" text,
    "select" text,
    "has""quote" text
);
COMMENT ON TABLE "boa_fx_ünïcødé"."Kunden" IS 'Kunden mit Umlauten — 顧客';
INSERT INTO "boa_fx_ünïcødé"."Kunden" VALUES
    (1, 180.5, '山田太郎', 'Hauptstraße 1', '🐘', 'MiXeD', 'reserved word', 'a "quoted" value'),
    (2, 165.0, 'Zoë Ångström', 'Øresundsvej 2', '🦀🐘', NULL, 'select', NULL),
    (3, NULL, 'Σωκράτης', 'Οδός 3', NULL, 'x', NULL, ''),
    (4, 171.2, 'محمد', 'شارع 4', '👩‍💻', 'rtl', 'from', 'tab' || E'\t' || 'inside'),
    (5, 159.9, 'සිංහල', 'කොළඹ 5', '🌴', 'si', 'where', 'line' || E'\n' || 'break');

CREATE TABLE "boa_fx_ünïcødé"."Bestellungen_ä" (
    "Nr" integer PRIMARY KEY,
    "Kunde" integer NOT NULL REFERENCES "boa_fx_ünïcødé"."Kunden" ("Nr"),
    "Betrag €" numeric(10, 2) CHECK ("Betrag €" >= 0)
);
INSERT INTO "boa_fx_ünïcødé"."Bestellungen_ä"
SELECT g, g % 5 + 1, g * 9.99 FROM generate_series(1, 25) g;

CREATE TABLE "boa_fx_ünïcødé"."表" ("列" text, "值" integer);  -- no PK
INSERT INTO "boa_fx_ünïcødé"."表" VALUES ('一', 1), ('二', 2), ('三', 3);

CREATE VIEW "boa_fx_ünïcødé"."Ansicht ö" AS
    SELECT k."名前", count(b."Nr") AS "Anzahl"
    FROM "boa_fx_ünïcødé"."Kunden" k LEFT JOIN "boa_fx_ünïcødé"."Bestellungen_ä" b ON b."Kunde" = k."Nr"
    GROUP BY k."名前";

-- Read-only role. ---------------------------------------------------------------
GRANT USAGE ON SCHEMA boa_fx, boa_fx_many, "boa_fx_ünïcødé", boa_fx_empty TO boa_ro;
GRANT SELECT ON ALL TABLES IN SCHEMA boa_fx, boa_fx_many, "boa_fx_ünïcødé" TO boa_ro;
GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA boa_fx TO boa_ro;

COMMIT;

-- Planner statistics so row-count estimates (explorer) are filled in.
ANALYZE boa_fx.all_types, boa_fx.products, boa_fx.counters, boa_fx.bookings, boa_fx.regions,
        boa_fx.stores, boa_fx.employees, boa_fx.order_heads, boa_fx.order_lines,
        boa_fx.event_log, boa_fx.settings_kv, boa_fx.measurements, boa_fx.daily_lines,
        "boa_fx_ünïcødé"."Kunden", "boa_fx_ünïcødé"."Bestellungen_ä", "boa_fx_ünïcødé"."表";
SELECT format('ANALYZE %I.%I', n.nspname, c.relname)
FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE n.nspname = 'boa_fx_many' AND c.relkind = 'r'
ORDER BY c.relname \gexec

-- ---------------------------------------------------------------------------
-- boa_dev2 (second database for the database switcher / per-database sessions)
-- ---------------------------------------------------------------------------
\connect boa_dev2
\set QUIET on
SET client_min_messages = warning;

BEGIN;

DROP SCHEMA IF EXISTS boa_fx2 CASCADE;
DROP TABLE IF EXISTS public.stock;
DROP TABLE IF EXISTS public.products;

CREATE TABLE public.products (
    id integer PRIMARY KEY,
    name text NOT NULL,
    category text NOT NULL DEFAULT 'misc',
    price numeric(10, 2) NOT NULL CHECK (price >= 0)
);
COMMENT ON TABLE public.products IS 'boa_dev2 fixture (same name as boa_fx.products, different shape)';
INSERT INTO public.products
SELECT g, 'Thing ' || g, (ARRAY['tools', 'food', 'books'])[g % 3 + 1], g * 1.5
FROM generate_series(1, 120) g;

CREATE TABLE public.stock (
    product_id integer NOT NULL REFERENCES public.products (id),
    warehouse text NOT NULL,
    qty integer NOT NULL DEFAULT 0,
    PRIMARY KEY (product_id, warehouse)
);
INSERT INTO public.stock
SELECT p, w, (p * 13 + length(w)) % 97
FROM generate_series(1, 120) p, unnest(ARRAY['north', 'south']) w;

CREATE SCHEMA boa_fx2;
CREATE TABLE boa_fx2.notes (
    id bigint GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY,
    body text NOT NULL,
    meta jsonb NOT NULL DEFAULT '{}'
);
INSERT INTO boa_fx2.notes (body, meta)
SELECT 'note ' || g, jsonb_build_object('db', 'boa_dev2', 'n', g) FROM generate_series(1, 40) g;
CREATE VIEW boa_fx2.stock_value AS
    SELECT p.id, p.name, sum(s.qty * p.price) AS value
    FROM public.products p JOIN public.stock s ON s.product_id = p.id
    GROUP BY p.id, p.name;

GRANT USAGE ON SCHEMA public, boa_fx2 TO boa_ro;
GRANT SELECT ON public.products, public.stock TO boa_ro;
GRANT SELECT ON ALL TABLES IN SCHEMA boa_fx2 TO boa_ro;

COMMIT;

ANALYZE public.products, public.stock, boa_fx2.notes;
