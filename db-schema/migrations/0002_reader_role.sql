-- cspell:word bypassrls
-- cspell:word nobypassrls
-- cspell:word noreplication
-- cspell:word rolname
-- The shared read-only role for the `dropset` database
-- (docs/data-feeds.md §8).
--
-- §8's ownership rule is "one writer per table, unrestricted readers", and
-- until now "reader" was a convention rather than an identity: every
-- consumer connected as `dropset`, the role that owns every table, so
-- nothing structurally stopped a dashboard query from writing one. This
-- gives readers a login that *cannot* write, which is what lets a read-only
-- consumer be pointed at the shared database without handing it the keys.
-- Grafana (market-data/grafana/) is the first such consumer.
--
-- Deliberately one general `dropset_ro`, not a role per consumer: the
-- privilege set a dashboard needs is exactly the one every other reader
-- needs, so per-consumer roles would multiply grants without buying any
-- isolation — they would all hold identical rights over identical tables.
--
-- Unlike 0001's plain `CREATE TABLE`, the role creation below IS guarded,
-- and that is a consequence of scope rather than a softening of the
-- single-schema-owner stance. A role is **cluster-wide**, not
-- per-database, so `dropset_ro` may already exist for a reason that is not
-- drift in *this* database: a second `dropset` database in the same cluster
-- (a staging copy, or a dump restored under another name) whose migration
-- ran first. An unguarded `CREATE ROLE` would fail that second run on an
-- object it is perfectly happy to share. Table DDL has no equivalent case,
-- so 0001's reasoning is untouched.
--
-- The password is a throwaway for the local compose stack, like the
-- hardcoded `dropset` superuser password beside it in
-- infra/localnet/docker-compose.yml. A real deployment rotates it out of
-- band (`ALTER ROLE dropset_ro PASSWORD …`, from the secret store) rather
-- than by editing this file: migrations are additive-only and this one has
-- already been applied, so an edit here would only change what a *fresh*
-- database gets.
-- The attributes are spelled out rather than left to defaults, and the
-- adopted branch floors them, because the guard above means this migration
-- may be handing `SELECT` to a role it did **not** create. A `dropset_ro`
-- provisioned by whoever set up that other database could carry
-- SUPERUSER or BYPASSRLS, and Grafana then logs in as it — so the ELSE
-- re-asserts the ceiling instead of trusting the name. It deliberately
-- does not touch the password: rotation is an out-of-band concern and
-- resetting it here would break whatever is already using the role.
DO $$
BEGIN
    IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'dropset_ro') THEN
        CREATE ROLE dropset_ro LOGIN PASSWORD 'dropset_ro'
            NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
    ELSE
        ALTER ROLE dropset_ro
            NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
    END IF;
END
$$;

-- `current_database()` rather than a literal `dropset`, because the grant
-- has to name the database it is running against and that name is not
-- fixed: the schema-fence test suite migrates a throwaway container's
-- default `postgres` database, and a restore may land under another name.
-- A literal would fail both. (CONNECT is granted to PUBLIC by default, so
-- this is close to a no-op today; it is stated explicitly so the role keeps
-- working if that default is ever revoked as a hardening step.)
DO $$
BEGIN
    EXECUTE format(
        'GRANT CONNECT ON DATABASE %I TO dropset_ro', current_database()
    );
END
$$;

GRANT USAGE ON SCHEMA public TO dropset_ro;

-- SELECT on what 0001 created, then the same for every table a later
-- migration adds, so a new market-data table shows up in a dashboard
-- without a follow-up grant. The default-privileges form is scoped to the
-- role that runs it, so it covers future tables only while migrations keep
-- being applied by this same role — which the one-schema-owner rule
-- already requires. A migration run as some other role would create tables
-- the reader cannot see, and that would surface as an empty panel.
--
-- Note what the blanket grant implies: every table a later migration puts
-- in `public` becomes readable by whatever holds these credentials, with no
-- per-table decision. That is right for market data and wrong for secrets,
-- so a table holding an API key, a feed credential, or position data does
-- not belong in `public` — give it its own schema, or revoke `SELECT` on it
-- from `dropset_ro` in the same migration that creates it.
GRANT SELECT ON ALL TABLES IN SCHEMA public TO dropset_ro;
ALTER DEFAULT PRIVILEGES IN SCHEMA public
    GRANT SELECT ON TABLES TO dropset_ro;
