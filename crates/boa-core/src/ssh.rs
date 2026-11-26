//! SSH tunnels. A connection with a tunnel reaches its server through the
//! system ssh (`/usr/bin/ssh -N -L 127.0.0.1:<free port>:<db host>:<db port>`),
//! so `~/.ssh/config` (host aliases, `ProxyJump`, `IdentityFile`), the agent
//! and `known_hosts` all apply. It runs in batch mode, without prompts: the
//! host key must already be known and a key with a passphrase must be in the
//! agent.
//!
//! A `Tunnel` owns the ssh process and kills it when dropped; `Pg` keeps one
//! per open connection and drops it on disconnect, on a failed connect and at
//! quit.

use crate::connections::{SshAuth, SshTunnel};
use crate::error::{CmdError, CmdResult};
use std::net::{Ipv4Addr, TcpListener};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

const SSH: &str = "/usr/bin/ssh";
/// How long ssh gets to connect, authenticate and open the forwarded port.
const READY_TIMEOUT: Duration = Duration::from_secs(15);
/// The tail of ssh's stderr kept for error messages.
const STDERR_MAX: usize = 8 * 1024;

/// A running ssh process forwarding `127.0.0.1:port` to the database server.
#[derive(Debug)]
pub struct Tunnel {
    child: Mutex<Child>,
    /// Local port forwarded to the database server.
    pub port: u16,
    /// The tail of ssh's stderr.
    stderr: Arc<Mutex<String>>,
}

impl Tunnel {
    /// Starts ssh and waits until the forwarded port accepts connections
    /// (ssh opens it once authenticated). `db_host` and `db_port` are the
    /// database as the SSH server sees it (`localhost`: the SSH server itself).
    pub async fn open(t: &SshTunnel, db_host: &str, db_port: u16) -> CmdResult<Self> {
        let key = t.identity_file.as_deref().map_or("", str::trim);
        if t.auth == SshAuth::KeyFile && !key.is_empty() && !expand_home(key).exists() {
            // ssh only warns about a missing -i file and carries on.
            return Err(CmdError::msg(format!(
                "SSH tunnel: key file {key} not found"
            )));
        }
        let port = free_port()?;
        start(SSH, &args(t, port, db_host, db_port)?, port, READY_TIMEOUT).await
    }

    /// False once ssh has exited (connection lost, server gone).
    pub fn is_alive(&self) -> bool {
        let mut child = self.child.lock().unwrap_or_else(PoisonError::into_inner);
        matches!(child.try_wait(), Ok(None))
    }

    /// Kills ssh now (dropping the tunnel does too).
    pub fn kill(&self) {
        let mut child = self.child.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = child.start_kill();
    }

    /// Adds what ssh reported to a database connection error, typically the
    /// SSH server failing to reach the database (the client only sees the
    /// connection closed).
    pub async fn explain(&self, mut e: CmdError) -> CmdError {
        // ssh logs before closing the forwarded connection; give the reader
        // a moment to catch up.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let stderr = self.stderr.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(line) = stderr.lines().rev().find(|l| l.contains("open failed")) {
            e.message = format!("{} (SSH tunnel: {})", e.message, line.trim());
        }
        e
    }

    #[cfg(test)]
    fn pid(&self) -> Option<u32> {
        self.child.lock().unwrap().id()
    }
}

/// ssh's arguments for forwarding `127.0.0.1:local_port` to `db_host:db_port`
/// (a host name, IP, or Unix socket directory on the SSH server).
pub fn args(t: &SshTunnel, local_port: u16, db_host: &str, db_port: u16) -> CmdResult<Vec<String>> {
    let host = t.host.trim();
    let user = t.user.trim();
    if host.is_empty() {
        return Err(CmdError::msg("SSH tunnel: the SSH host is empty"));
    }
    if host.starts_with('-') || user.starts_with('-') {
        return Err(CmdError::msg("SSH tunnel: invalid SSH host or user"));
    }
    let db_host = db_host.trim();
    let target = if db_host.starts_with('/') {
        format!("{}/.s.PGSQL.{db_port}", db_host.trim_end_matches('/'))
    } else if db_host.contains(':') {
        format!("[{db_host}]:{db_port}")
    } else if db_host.is_empty() {
        format!("localhost:{db_port}")
    } else {
        format!("{db_host}:{db_port}")
    };
    let mut a: Vec<String> = [
        "-N",
        "-T",
        "-o",
        "ExitOnForwardFailure=yes",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=10",
        "-o",
        "ServerAliveInterval=30",
        "-o",
        "ServerAliveCountMax=3",
        // A connection of its own: a forward added to a shared ControlMaster
        // connection would outlive this process.
        "-o",
        "ControlMaster=no",
        "-o",
        "ControlPath=none",
        "-L",
    ]
    .map(String::from)
    .into();
    a.push(format!("127.0.0.1:{local_port}:{target}"));
    if let Some(port) = t.port {
        a.extend(["-p".into(), port.to_string()]);
    }
    if !user.is_empty() {
        a.extend(["-l".into(), user.into()]);
    }
    if t.auth == SshAuth::KeyFile {
        let key = t.identity_file.as_deref().map_or("", str::trim);
        if key.is_empty() {
            return Err(CmdError::msg("SSH tunnel: choose a private key file"));
        }
        a.extend(["-i".into(), key.into()]);
    }
    a.extend(["--".into(), host.into()]);
    Ok(a)
}

fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(path),
    }
}

/// A local port nobody listens on: bound and released for ssh to bind
/// (another process taking it first makes ssh exit: `ExitOnForwardFailure`).
fn free_port() -> CmdResult<u16> {
    Ok(TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?
        .local_addr()?
        .port())
}

async fn start(program: &str, args: &[String], port: u16, timeout: Duration) -> CmdResult<Tunnel> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| CmdError::msg(format!("SSH tunnel: could not run {program}: {e}")))?;
    // Drained for as long as ssh runs, so a full pipe never blocks it.
    let stderr = Arc::new(Mutex::new(String::new()));
    let drain = child.stderr.take().map(|pipe| {
        let buf = stderr.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(pipe).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!("ssh: {line}");
                let mut buf = buf.lock().unwrap_or_else(PoisonError::into_inner);
                buf.push_str(&line);
                buf.push('\n');
                while buf.len() > STDERR_MAX {
                    let cut = buf.find('\n').map_or(buf.len(), |i| i + 1);
                    buf.drain(..cut);
                }
            }
        })
    });
    let stderr_text = || {
        stderr
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    };
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            // Let the reader catch up with what ssh printed before exiting.
            if let Some(drain) = drain {
                let _ = tokio::time::timeout(Duration::from_secs(1), drain).await;
            }
            return Err(failure(&stderr_text(), &format!("ssh exited ({status})")));
        }
        if tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .is_ok()
        {
            return Ok(Tunnel {
                child: Mutex::new(child),
                port,
                stderr,
            });
        }
        if started.elapsed() >= timeout {
            let _ = child.kill().await;
            return Err(failure(
                &stderr_text(),
                &format!("timed out after {} s", timeout.as_secs_f32()),
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The error for a tunnel that didn't come up: ssh's last lines, with what
/// to do about the common causes.
fn failure(stderr: &str, fallback: &str) -> CmdError {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("Warning: Permanently added"))
        .collect();
    let mut message = format!(
        "SSH tunnel: {}",
        if lines.is_empty() {
            fallback.to_owned()
        } else {
            lines[lines.len().saturating_sub(3)..].join(" ")
        }
    );
    let tip = if stderr.contains("REMOTE HOST IDENTIFICATION HAS CHANGED") {
        Some("The SSH server's host key changed: check why before fixing ~/.ssh/known_hosts.")
    } else if stderr.contains("Host key verification failed") {
        Some("Connect once with ssh in Terminal to check and accept the server's host key.")
    } else if stderr.contains("Permission denied") {
        Some(
            "Add the key to the SSH agent (ssh-add) or pick a key file; \
             keys with a passphrase must be in the agent.",
        )
    } else {
        None
    };
    if let Some(tip) = tip {
        message.truncate(message.trim_end_matches('.').len());
        message.push_str(". ");
        message.push_str(tip);
    }
    CmdError::msg(message)
}

#[cfg(test)]
mod tests {
    use super::{SSH, SshAuth, SshTunnel, Tunnel, args, failure, free_port, start};
    use std::path::Path;
    use std::time::Duration;

    fn tunnel() -> SshTunnel {
        SshTunnel {
            enabled: true,
            host: "bastion".into(),
            ..SshTunnel::default()
        }
    }

    #[test]
    fn builds_ssh_arguments() {
        let a = args(&tunnel(), 5555, "db.internal", 5432).unwrap();
        assert_eq!(a[0], "-N");
        assert!(a.windows(2).any(|w| w == ["-o", "BatchMode=yes"]));
        assert!(
            a.windows(2)
                .any(|w| w == ["-o", "ExitOnForwardFailure=yes"])
        );
        assert!(a.windows(2).any(|w| w == ["-o", "ServerAliveInterval=30"]));
        assert!(
            a.windows(2)
                .any(|w| w == ["-L", "127.0.0.1:5555:db.internal:5432"])
        );
        // Port and user left to ~/.ssh/config; the host comes last.
        assert!(!a.iter().any(|x| x == "-p" || x == "-l" || x == "-i"));
        assert_eq!(a[a.len() - 2..], ["--", "bastion"]);

        let t = SshTunnel {
            host: " bastion.example.com ".into(),
            port: Some(2222),
            user: "deploy".into(),
            auth: SshAuth::KeyFile,
            identity_file: Some("~/.ssh/id_ed25519".into()),
            ..tunnel()
        };
        let a = args(&t, 5555, "localhost", 6543).unwrap();
        assert!(
            a.windows(2)
                .any(|w| w == ["-L", "127.0.0.1:5555:localhost:6543"])
        );
        assert!(a.windows(2).any(|w| w == ["-p", "2222"]));
        assert!(a.windows(2).any(|w| w == ["-l", "deploy"]));
        assert!(a.windows(2).any(|w| w == ["-i", "~/.ssh/id_ed25519"]));
        assert_eq!(a[a.len() - 2..], ["--", "bastion.example.com"]);

        // IPv6 and Unix-socket database hosts.
        let a = args(&tunnel(), 1, "::1", 5432).unwrap();
        assert!(a.contains(&"127.0.0.1:1:[::1]:5432".to_owned()));
        let a = args(&tunnel(), 1, "/var/run/postgresql/", 5432).unwrap();
        assert!(a.contains(&"127.0.0.1:1:/var/run/postgresql/.s.PGSQL.5432".to_owned()));
    }

    #[test]
    fn rejects_bad_settings() {
        let no_host = SshTunnel {
            host: " ".into(),
            ..tunnel()
        };
        assert!(args(&no_host, 1, "h", 1).is_err());
        let option = SshTunnel {
            host: "-oProxyCommand=x".into(),
            ..tunnel()
        };
        assert!(args(&option, 1, "h", 1).is_err());
        let no_key = SshTunnel {
            auth: SshAuth::KeyFile,
            ..tunnel()
        };
        assert!(args(&no_key, 1, "h", 1).is_err());
    }

    #[test]
    fn explains_common_failures() {
        let e = failure(
            "Warning: Permanently added 'x' (ED25519) to the list of known hosts.\n\
             deploy@x: Permission denied (publickey).\n",
            "",
        );
        assert!(
            e.message
                .starts_with("SSH tunnel: deploy@x: Permission denied (publickey).")
        );
        assert!(e.message.contains("ssh-add"));
        let e = failure("Host key verification failed.\n", "");
        assert!(
            e.message
                .starts_with("SSH tunnel: Host key verification failed. Connect once"),
            "{}",
            e.message
        );
        assert_eq!(failure("", "ssh exited").message, "SSH tunnel: ssh exited");
    }

    /// Whether the process is gone (or a zombie waiting to be reaped).
    fn gone(pid: u32) -> bool {
        let out = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        let stat = String::from_utf8_lossy(&out.stdout);
        stat.trim().is_empty() || stat.trim().starts_with('Z')
    }

    #[tokio::test]
    async fn unreachable_ssh_server_fails_cleanly() {
        if !Path::new(SSH).exists() {
            return;
        }
        // Nothing listens on port 1; -F /dev/null ignores the user's config.
        let t = SshTunnel {
            host: "127.0.0.1".into(),
            port: Some(1),
            ..tunnel()
        };
        let port = free_port().unwrap();
        let mut a = vec!["-F".to_owned(), "/dev/null".to_owned()];
        a.extend(args(&t, port, "localhost", 5432).unwrap());
        let e = start(SSH, &a, port, Duration::from_secs(10))
            .await
            .unwrap_err();
        assert!(e.message.starts_with("SSH tunnel: "), "{}", e.message);
        assert!(e.message.contains("Connection refused"), "{}", e.message);
    }

    #[tokio::test]
    async fn timeout_kills_the_process() {
        let marker = "31.4159";
        let e = start(
            "/bin/sleep",
            &[marker.into()],
            free_port().unwrap(),
            Duration::from_millis(300),
        )
        .await
        .unwrap_err();
        assert!(e.message.contains("timed out"), "{}", e.message);
        let out = std::process::Command::new("pgrep")
            .args(["-f", &format!("sleep {marker}")])
            .output()
            .unwrap();
        assert!(out.stdout.is_empty(), "sleep still running");
    }

    #[tokio::test]
    async fn ready_tunnel_is_killed_on_drop() {
        // A stand-in for ssh: something listening on the port.
        let port = free_port().unwrap();
        let t: Tunnel = start(
            "/usr/bin/nc",
            &["-lk".into(), "127.0.0.1".into(), port.to_string()],
            port,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert!(t.is_alive());
        let pid = t.pid().unwrap();
        drop(t);
        for _ in 0..20 {
            if gone(pid) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("process {pid} still running");
    }
}
