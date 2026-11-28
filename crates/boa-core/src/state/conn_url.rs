//! The connection dialog's pure logic: ported from
//! `ui/src/components/ConnectionDialog.tsx` (`parseUrl`, `cleanSsh`,
//! `sshValid`, the form's `blank` / `named` / `valid`) and its tests
//! (`ConnectionDialog.test.ts`), mapped to `crate::connections::ConnConfig`.
//!
//! The URL field (new connections only) fills the form from a pasted
//! `postgres://user:password@host:port/database?sslmode=…` (or
//! `redis://user:password@host:port/db`, `rediss://` for TLS) as you type; a
//! string that is not such a URL changes nothing. `parse_url` follows what
//! the WHATWG `URL` parser did for these (non-special) schemes: the last `@`
//! ends the user info, the first `:` in it splits user and password, the
//! authority ends at `/`, `?` or `#`, parts are percent-decoded, the port
//! defaults to 5432 and the host to `localhost`.

use crate::connections::{ConnConfig, DbKind, SshAuth, SshTunnel, SslMode};

/// What a pasted URL sets (`{ config: Partial<ConnConfig>; password? }`):
/// host and port always, the rest only when the URL has them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedUrl {
    /// `postgres(ql)://`: PostgreSQL, `redis://` / `rediss://`: Redis.
    pub kind: DbKind,
    pub host: String,
    pub port: u16,
    pub user: Option<String>,
    pub database: Option<String>,
    pub ssl_mode: Option<SslMode>,
    pub password: Option<String>,
}

impl ParsedUrl {
    /// Fills the form's fields (`set(parsed.config)`) and returns the
    /// password to put in the password field (and mark as typed), if any.
    pub fn apply(&self, cfg: &mut ConnConfig) -> Option<String> {
        switch_kind(cfg, self.kind);
        cfg.host.clone_from(&self.host);
        cfg.port = self.port;
        if let Some(user) = &self.user {
            cfg.user.clone_from(user);
        }
        if let Some(db) = &self.database {
            cfg.database.clone_from(db);
        }
        if let Some(mode) = self.ssl_mode {
            cfg.ssl_mode = mode;
        }
        self.password.clone()
    }
}

/// `decodeURIComponent`: `None` for a bad escape or invalid UTF-8 (where
/// JavaScript threw).
fn decode_component(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            if !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `application/x-www-form-urlencoded` decoding of one name or value
/// (`URLSearchParams`): `+` is a space, bad escapes stay as written.
fn decode_form(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if s
                .get(i + 1..i + 3)
                .is_some_and(|h| h.bytes().all(|c| c.is_ascii_hexdigit())) =>
            {
                out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap_or(b'%'));
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `searchParams.get(name)`: the first value of `name` in a query string.
fn query_param(query: &str, name: &str) -> Option<String> {
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| p.split_once('=').unwrap_or((p, "")))
        .find(|(k, _)| decode_form(k) == name)
        .map(|(_, v)| decode_form(v))
}

/// Characters an (opaque) URL host may not contain.
fn forbidden_in_host(c: char) -> bool {
    matches!(
        c,
        '\0' | '\t'
            | '\n'
            | '\r'
            | ' '
            | '#'
            | '/'
            | ':'
            | '<'
            | '>'
            | '?'
            | '@'
            | '['
            | '\\'
            | ']'
            | '^'
            | '|'
    )
}

/// Fills the fields from `postgres://user:pass@host:port/db?sslmode=…`
/// (`postgresql://` too); `None` for other schemes and anything that is not
/// a URL. `sslmode` `verify-ca` maps to `verify-full` (the only verifying
/// mode Boa has); unknown modes are ignored. (`parseUrl`)
pub fn parse_url(url: &str) -> Option<ParsedUrl> {
    // The URL parser trims the ends and drops tabs and newlines anywhere.
    let cleaned: String = url
        .trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}' || c.is_ascii_control())
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    let (scheme, rest) = cleaned.split_once(':')?;
    let mut chars = scheme.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        || !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return None;
    }
    let scheme = scheme.to_ascii_lowercase();
    let kind = match scheme.as_str() {
        "postgres" | "postgresql" => DbKind::Postgres,
        "redis" | "rediss" => DbKind::Redis,
        _ => return None,
    };
    let rest = rest.split('#').next().unwrap_or("");
    let (before_query, query) = rest.split_once('?').unwrap_or((rest, ""));

    let (authority, path) = match before_query.strip_prefix("//") {
        Some(after) => {
            let end = after.find('/').unwrap_or(after.len());
            (Some(&after[..end]), &after[end..])
        }
        None => (None, before_query),
    };

    let (mut user, mut password, mut host, mut port) = (None, None, String::new(), None);
    if let Some(authority) = authority {
        let (userinfo, hostport) = match authority.rfind('@') {
            Some(at) => (Some(&authority[..at]), &authority[at + 1..]),
            None => (None, authority),
        };
        if let Some(info) = userinfo {
            if hostport.is_empty() {
                return None;
            }
            let (u, p) = match info.split_once(':') {
                Some((u, p)) => (u, Some(p)),
                None => (info, None),
            };
            if !u.is_empty() {
                user = Some(decode_component(u)?);
            }
            if let Some(p) = p.filter(|p| !p.is_empty()) {
                password = Some(decode_component(p)?);
            }
        }
        let (h, port_text) = if let Some(v6) = hostport.strip_prefix('[') {
            let close = v6.find(']')?;
            let addr = &v6[..close];
            if addr.is_empty()
                || !addr
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
            {
                return None;
            }
            let after = &v6[close + 1..];
            let port_text = match after.strip_prefix(':') {
                Some(p) => Some(p),
                None if after.is_empty() => None,
                None => return None,
            };
            // Brackets dropped: Postgres wants the bare address.
            (addr.to_owned(), port_text)
        } else {
            let (h, p) = match hostport.split_once(':') {
                Some((h, p)) => (h, Some(p)),
                None => (hostport, None),
            };
            if h.chars().any(forbidden_in_host) {
                return None;
            }
            (decode_component(h)?, p)
        };
        host = h;
        if let Some(p) = port_text.filter(|p| !p.is_empty()) {
            if !p.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let n: u32 = p.trim_start_matches('0').parse().unwrap_or(0);
            port = Some(u16::try_from(n).ok()?);
        }
    }

    let db = decode_component(path.strip_prefix('/').unwrap_or(path))?;
    if kind == DbKind::Redis && !db.is_empty() && crate::redis::db_index(&db).is_none() {
        return None;
    }
    let ssl_mode = match query_param(query, "sslmode").as_deref() {
        _ if scheme == "rediss" => Some(SslMode::Require),
        _ if kind == DbKind::Redis => Some(SslMode::Disable),
        Some("disable") => Some(SslMode::Disable),
        Some("prefer") => Some(SslMode::Prefer),
        Some("require") => Some(SslMode::Require),
        Some("verify-full" | "verify-ca") => Some(SslMode::VerifyFull),
        _ => None,
    };
    Some(ParsedUrl {
        kind,
        host: if host.is_empty() {
            "localhost".into()
        } else {
            host
        },
        port: port.unwrap_or(kind.default_port()),
        user,
        database: (!db.is_empty()).then_some(db),
        ssl_mode,
        password,
    })
}

/// JavaScript's `trim` (Unicode white space and the BOM).
fn trim(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
}

/// Tunnel settings to save: trimmed, empty optional fields left out; none
/// if never filled in (disabled with no host, user or key). (`cleanSsh`)
pub fn clean_ssh(ssh: Option<&SshTunnel>) -> Option<SshTunnel> {
    let ssh = ssh?;
    let host = trim(&ssh.host).to_owned();
    let user = trim(&ssh.user).to_owned();
    let identity_file = ssh
        .identity_file
        .as_deref()
        .map(trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    if !ssh.enabled && host.is_empty() && user.is_empty() && identity_file.is_none() {
        return None;
    }
    Some(SshTunnel {
        enabled: ssh.enabled,
        host,
        port: ssh.port.filter(|&p| p != 0),
        user,
        auth: ssh.auth,
        identity_file,
    })
}

/// An enabled tunnel needs a host, a valid port and, for key-file auth, a
/// key. A disabled (or absent) one is always valid. (`sshValid`)
pub fn ssh_valid(ssh: Option<&SshTunnel>) -> bool {
    let Some(ssh) = ssh.filter(|s| s.enabled) else {
        return true;
    };
    if trim(&ssh.host).is_empty() || ssh.port == Some(0) {
        return false;
    }
    ssh.auth != SshAuth::KeyFile
        || ssh
            .identity_file
            .as_deref()
            .is_some_and(|k| !trim(k).is_empty())
}

/// A new connection's form (`blank()`): localhost:5432, user and database
/// `postgres`, `prefer` TLS. `id`: a fresh id (the TS used `crypto.randomUUID()`).
pub fn blank_config(id: String) -> ConnConfig {
    ConnConfig {
        kind: Default::default(),
        id,
        name: String::new(),
        host: "localhost".into(),
        port: 5432,
        user: "postgres".into(),
        database: "postgres".into(),
        ssl_mode: SslMode::Prefer,
        read_only: false,
        environment: None,
        color: None,
        ssh: None,
    }
}

/// A new Redis connection's form: localhost:6379, no user, database 0, no TLS.
pub fn blank_redis_config(id: String) -> ConnConfig {
    ConnConfig {
        kind: DbKind::Redis,
        port: DbKind::Redis.default_port(),
        user: String::new(),
        database: "0".into(),
        ssl_mode: SslMode::Disable,
        ..blank_config(id)
    }
}

/// Switches the form to `kind`, swapping fields that still hold the other
/// kind's defaults (port, user, database, TLS mode).
pub fn switch_kind(cfg: &mut ConnConfig, kind: DbKind) {
    if cfg.kind == kind {
        return;
    }
    let (from, to) = (blank_kind(cfg.kind), blank_kind(kind));
    if cfg.port == from.port || cfg.port == 0 {
        cfg.port = to.port;
    }
    if trim(&cfg.user) == from.user {
        cfg.user = to.user;
    }
    if trim(&cfg.database) == from.database || trim(&cfg.database).is_empty() {
        cfg.database = to.database;
    }
    if cfg.ssl_mode == from.ssl_mode || (kind == DbKind::Redis && cfg.ssl_mode == SslMode::Prefer) {
        cfg.ssl_mode = to.ssl_mode;
    }
    cfg.kind = kind;
}

fn blank_kind(kind: DbKind) -> ConnConfig {
    match kind {
        DbKind::Postgres => blank_config(String::new()),
        DbKind::Redis => blank_redis_config(String::new()),
    }
}

/// The default database a kind connects to when the field is empty.
fn default_database(kind: DbKind) -> &'static str {
    match kind {
        DbKind::Postgres => "postgres",
        DbKind::Redis => "0",
    }
}

/// The Name field's placeholder: `<database or postgres>@<host or localhost>`
/// (Redis: `db<index>@<host>`).
pub fn name_placeholder(cfg: &ConnConfig) -> String {
    let redis_db;
    let db = if cfg.kind == DbKind::Redis {
        redis_db = crate::redis::db_name(crate::redis::db_index(&cfg.database).unwrap_or(0));
        redis_db.as_str()
    } else if cfg.database.is_empty() {
        default_database(cfg.kind)
    } else {
        &cfg.database
    };
    let host = if cfg.host.is_empty() {
        "localhost"
    } else {
        &cfg.host
    };
    format!("{db}@{host}")
}

/// The config to test or save (`named()`): host and user trimmed, the
/// database `postgres` when empty, the name `<database>@<host>` when empty
/// (from the fields as typed), the tunnel cleaned ([`clean_ssh`]).
pub fn named(cfg: &ConnConfig) -> ConnConfig {
    let mut out = cfg.clone();
    out.name = match trim(&cfg.name) {
        "" if cfg.kind == DbKind::Redis => name_placeholder(cfg),
        "" => format!("{}@{}", cfg.database, cfg.host),
        name => name.to_owned(),
    };
    out.host = trim(&cfg.host).to_owned();
    out.user = trim(&cfg.user).to_owned();
    out.database = match trim(&cfg.database) {
        "" => default_database(cfg.kind).into(),
        db => db.to_owned(),
    };
    out.ssh = clean_ssh(cfg.ssh.as_ref());
    out
}

/// Test / Save are enabled: a host, a user (PostgreSQL; optional for Redis,
/// whose database must be an index), a port in 1..=65535 and a valid tunnel
/// (`valid`).
pub fn form_valid(cfg: &ConnConfig) -> bool {
    let kind_ok = match cfg.kind {
        DbKind::Postgres => !trim(&cfg.user).is_empty(),
        DbKind::Redis => {
            trim(&cfg.database).is_empty() || crate::redis::db_index(&cfg.database).is_some()
        }
    };
    !trim(&cfg.host).is_empty() && kind_ok && cfg.port > 0 && ssh_valid(cfg.ssh.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redis_urls_and_forms() {
        let p = parse_url("redis://:s3cret@cache.example.com/3").unwrap();
        assert_eq!(p.kind, DbKind::Redis);
        assert_eq!((p.host.as_str(), p.port), ("cache.example.com", 6379));
        assert_eq!(p.user, None);
        assert_eq!(p.password.as_deref(), Some("s3cret"));
        assert_eq!(p.database.as_deref(), Some("3"));
        assert_eq!(p.ssl_mode, Some(SslMode::Disable));
        let p = parse_url("rediss://app:pw@h:6380").unwrap();
        assert_eq!((p.port, p.ssl_mode), (6380, Some(SslMode::Require)));
        assert_eq!(p.user.as_deref(), Some("app"));
        assert!(parse_url("redis://h/notanindex").is_none());
        assert_eq!(parse_url("postgres://h/db").unwrap().kind, DbKind::Postgres);

        let mut cfg = blank_config("x".into());
        parse_url("redis://h:7000/2").unwrap().apply(&mut cfg);
        assert_eq!(cfg.kind, DbKind::Redis);

        let mut cfg = blank_config("x".into());
        switch_kind(&mut cfg, DbKind::Redis);
        assert_eq!(
            (cfg.port, cfg.user.as_str(), cfg.database.as_str()),
            (6379, "", "0")
        );
        assert_eq!(cfg.ssl_mode, SslMode::Disable);
        assert!(form_valid(&cfg), "no user needed for Redis");
        assert_eq!(name_placeholder(&cfg), "db0@localhost");
        assert_eq!(named(&cfg).name, "db0@localhost");
        cfg.port = 7000;
        cfg.database = "abc".into();
        assert!(!form_valid(&cfg));
        cfg.database = "5".into();
        switch_kind(&mut cfg, DbKind::Postgres);
        assert_eq!(
            (cfg.port, cfg.user.as_str(), cfg.database.as_str()),
            (7000, "postgres", "5")
        );
        assert_eq!(cfg.ssl_mode, SslMode::Prefer);
        assert!(!blank_redis_config("r".into()).kind.is_postgres());
    }

    fn ssh(enabled: bool, host: &str, user: &str, auth: SshAuth) -> SshTunnel {
        SshTunnel {
            enabled,
            host: host.into(),
            port: None,
            user: user.into(),
            auth,
            identity_file: None,
        }
    }

    // describe('SSH tunnel settings')

    #[test]
    fn saves_nothing_for_a_tunnel_never_filled_in() {
        assert_eq!(clean_ssh(None), None);
        assert_eq!(clean_ssh(Some(&ssh(false, " ", "", SshAuth::Agent))), None);
    }

    #[test]
    fn trims_and_leaves_out_empty_optional_fields() {
        let t = SshTunnel {
            identity_file: Some(" ".into()),
            ..ssh(true, " bastion ", " me ", SshAuth::Agent)
        };
        assert_eq!(
            clean_ssh(Some(&t)),
            Some(ssh(true, "bastion", "me", SshAuth::Agent))
        );
        // Disabled but filled in: kept, so turning it back on restores it.
        let t = SshTunnel {
            port: Some(2222),
            identity_file: Some("~/.ssh/k".into()),
            ..ssh(false, "b", "", SshAuth::KeyFile)
        };
        assert_eq!(clean_ssh(Some(&t)), Some(t.clone()));
        // And it saves as the TS did (no empty optional keys).
        assert_eq!(
            serde_json::to_string(&clean_ssh(Some(&ssh(true, "b ", "", SshAuth::Agent)))).unwrap(),
            r#"{"enabled":true,"host":"b","user":"","auth":"agent"}"#
        );
    }

    #[test]
    fn requires_a_host_and_a_key_file_for_key_file_auth() {
        assert!(ssh_valid(None));
        assert!(ssh_valid(Some(&ssh(false, "", "", SshAuth::KeyFile))));
        assert!(!ssh_valid(Some(&ssh(true, "", "", SshAuth::Agent))));
        assert!(ssh_valid(Some(&ssh(true, "b", "", SshAuth::Agent))));
        assert!(!ssh_valid(Some(&ssh(true, "b", "", SshAuth::KeyFile))));
        let with_key = SshTunnel {
            identity_file: Some("k".into()),
            ..ssh(true, "b", "", SshAuth::KeyFile)
        };
        assert!(ssh_valid(Some(&with_key)));
        // Port 0 is out of range (the TS checked 1..=65535).
        let port0 = SshTunnel {
            port: Some(0),
            ..ssh(true, "b", "", SshAuth::Agent)
        };
        assert!(!ssh_valid(Some(&port0)));
    }

    // describe('parseUrl')

    #[test]
    fn reads_every_part_of_a_postgres_url() {
        assert_eq!(
            parse_url("postgresql://app%40x:p%3Ass@db.example.com:6543/my%20db?sslmode=require"),
            Some(ParsedUrl {
                kind: DbKind::Postgres,
                host: "db.example.com".into(),
                port: 6543,
                user: Some("app@x".into()),
                database: Some("my db".into()),
                ssl_mode: Some(SslMode::Require),
                password: Some("p:ss".into()),
            })
        );
    }

    #[test]
    fn defaults_the_port_and_maps_verify_ca_to_verify_full() {
        assert_eq!(
            parse_url("postgres://localhost?sslmode=verify-ca"),
            Some(ParsedUrl {
                kind: DbKind::Postgres,
                host: "localhost".into(),
                port: 5432,
                user: None,
                database: None,
                ssl_mode: Some(SslMode::VerifyFull),
                password: None,
            })
        );
    }

    #[test]
    fn rejects_other_schemes_and_junk() {
        assert_eq!(parse_url("mysql://x"), None);
        assert_eq!(parse_url("not a url"), None);
    }

    // Beyond the TS cases: how the URL parser read the edges.

    #[test]
    fn url_edges() {
        let p = parse_url("  POSTGRES://u:p@w@h:05432/d?x=1&sslmode=disable&sslmode=require#frag ")
            .unwrap();
        assert_eq!(
            (
                p.user.as_deref(),
                p.password.as_deref(),
                p.host.as_str(),
                p.port
            ),
            (Some("u"), Some("p@w"), "h", 5432)
        );
        assert_eq!(p.database.as_deref(), Some("d"));
        assert_eq!(p.ssl_mode, Some(SslMode::Disable));
        // No host: localhost; an empty password is none; unknown modes are ignored.
        let p = parse_url("postgres://me:@/app?sslmode=allow");
        assert_eq!(p, None, "user info needs a host");
        let p = parse_url("postgres:///app?sslmode=allow").unwrap();
        assert_eq!(
            (p.host.as_str(), p.database.as_deref(), p.ssl_mode),
            ("localhost", Some("app"), None)
        );
        let p = parse_url("postgres://me:@h").unwrap();
        assert_eq!((p.user.as_deref(), p.password), (Some("me"), None));
        // IPv6: the address without brackets.
        let p = parse_url("postgresql://[::1]:5433/x").unwrap();
        assert_eq!((p.host.as_str(), p.port), ("::1", 5433));
        // Bad ports, hosts and escapes are not URLs.
        assert_eq!(parse_url("postgres://h:99999/x"), None);
        assert_eq!(parse_url("postgres://h:54a/x"), None);
        assert_eq!(parse_url("postgres://a b/x"), None);
        assert_eq!(parse_url("postgres://h/%zz"), None);
        assert_eq!(parse_url("1postgres://h"), None);
        // An empty port is the default one.
        assert_eq!(parse_url("postgres://h:/x").unwrap().port, 5432);
    }

    #[test]
    fn a_parsed_url_fills_the_form() {
        let mut cfg = blank_config("id1".into());
        let pw = parse_url("postgres://ann:secret@db:6000/sales?sslmode=require")
            .unwrap()
            .apply(&mut cfg);
        assert_eq!(pw.as_deref(), Some("secret"));
        assert_eq!(
            (
                cfg.host.as_str(),
                cfg.port,
                cfg.user.as_str(),
                cfg.database.as_str(),
                cfg.ssl_mode
            ),
            ("db", 6000, "ann", "sales", SslMode::Require)
        );
        // Only host and port when that is all there is.
        let pw = parse_url("postgres://other").unwrap().apply(&mut cfg);
        assert_eq!(pw, None);
        assert_eq!(
            (
                cfg.host.as_str(),
                cfg.port,
                cfg.user.as_str(),
                cfg.database.as_str()
            ),
            ("other", 5432, "ann", "sales")
        );
    }

    #[test]
    fn names_and_validates_the_form() {
        let mut cfg = blank_config("id1".into());
        assert_eq!(name_placeholder(&cfg), "postgres@localhost");
        cfg.host = " db ".into();
        cfg.database = String::new();
        cfg.user = " me ".into();
        cfg.ssh = Some(ssh(false, " ", "", SshAuth::Agent));
        let n = named(&cfg);
        assert_eq!(n.name, "@ db ");
        assert_eq!(
            (n.host.as_str(), n.user.as_str(), n.database.as_str()),
            ("db", "me", "postgres")
        );
        assert_eq!(n.ssh, None);
        cfg.name = " Prod ".into();
        assert_eq!(named(&cfg).name, "Prod");
        assert!(form_valid(&cfg));
        cfg.port = 0;
        assert!(!form_valid(&cfg));
        cfg.port = 5432;
        cfg.user = " ".into();
        assert!(!form_valid(&cfg));
        cfg.user = "me".into();
        cfg.ssh = Some(ssh(true, "", "", SshAuth::Agent));
        assert!(!form_valid(&cfg));
    }
}
