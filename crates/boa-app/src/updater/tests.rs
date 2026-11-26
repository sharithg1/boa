//! Signature checks, the feed over HTTP, unpacking and the bundle swap.
//!
//! The key and signatures below are a THROWAWAY minisign key generated with
//! `tauri signer generate` for these tests (password "throwaway"); it signs
//! nothing else. `PAYLOAD` was signed with `tauri signer sign`, once with
//! `--app-version 1.2.3` (SIG_V) and once without (SIG_NO_VERSION).

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use super::*;

pub(crate) const TEST_PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEM2MEE5NkFGMTUwNDVDODkKUldTSlhBUVZyNVlLeGczMGI1MThQSSthK1lHS0tBNzQyVGxSQkx3djZEZUJ0aVE3OXp4cEt2MnQK";
pub(crate) const PAYLOAD: &[u8] = b"boa updater test payload\n";
pub(crate) const SIG_V: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVTSlhBUVZyNVlLeG5RZGdSTE5sbEQxbnMySDFNL3dWaFBrWER0TlpHbktNeXIxYzVGdi9LazV5TjlqNGpOS0pLOWdRQ1NrRjFGMnF5YmJyaUFNMzVVU0NJN2JoNDNabkFrPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk1Njc2NDgwCWZpbGU6cGF5bG9hZC5iaW4JdmVyc2lvbjoxLjIuMwpVZWtRQkQ0TXg4OFc0ZzRoaUdiNG15cmFnTHJMUXFlTytZZzB5SC9iRnpjN0tuSEZuVDRaQjlaU3VpbDhSUi9PZ2NhK1FjbFFaQ3Zxb09mbWR0YXdBdz09Cg==";
const SIG_NO_VERSION: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVTSlhBUVZyNVlLeGdvN29keVVnZ3FnaDEyT1hCRlJTVi9OeE5tL2RIdUE1SlVpaWpWMmt5ZzFDaDdBZUw3eCtaSnZSV05QU1UyWU9vTEZoeUFFR016a05qaVM1WUZmYlFZPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk1Njc2NDczCWZpbGU6cGF5bG9hZC5iaW4KQWZqVWl0Zy9jN2E3VzdXV2pXVk1Kb3NEeWd5bUFaaC9SUVdEOUdFZVZvcTI0VnVHMElYR1M2eEU1ek1Mc2d5R01uUnpJRk9VWHVZWTVOTUk5a1JOQkE9PQo=";

// ── Signatures ───────────────────────────────────────────────────────────

#[test]
fn accepts_a_good_signature_for_the_announced_version() {
    verify_signature(PAYLOAD, SIG_V, TEST_PUBKEY, "1.2.3", true).unwrap();
    // Spelled differently, same SemVer.
    verify_signature(PAYLOAD, SIG_V, TEST_PUBKEY, "v1.2.3", true).unwrap();
    // latest.json written by jq --rawfile keeps a trailing newline.
    verify_signature(PAYLOAD, &format!("{SIG_V}\n"), TEST_PUBKEY, "1.2.3", true).unwrap();
}

#[test]
fn rejects_tampered_data() {
    let mut data = PAYLOAD.to_vec();
    data[0] ^= 1;
    let err = verify_signature(&data, SIG_V, TEST_PUBKEY, "1.2.3", true).unwrap_err();
    assert!(matches!(err, Error::Signature(_)), "{err}");
    let mut longer = PAYLOAD.to_vec();
    longer.push(b'x');
    assert!(matches!(
        verify_signature(&longer, SIG_V, TEST_PUBKEY, "1.2.3", true),
        Err(Error::Signature(_))
    ));
}

#[test]
fn rejects_a_signature_from_another_key() {
    // Signed with the throwaway key, checked against the production key.
    let err = verify_signature(PAYLOAD, SIG_V, PRODUCTION_PUBKEY, "1.2.3", true).unwrap_err();
    assert!(matches!(err, Error::Signature(_)), "{err}");
}

#[test]
fn rejects_a_signature_for_another_version() {
    let err = verify_signature(PAYLOAD, SIG_V, TEST_PUBKEY, "1.2.4", true).unwrap_err();
    assert!(
        matches!(&err, Error::SignedVersion { signed, announced } if signed == "1.2.3" && announced == "1.2.4"),
        "{err}"
    );
    // Even when a version is not required, a present one must match.
    assert!(verify_signature(PAYLOAD, SIG_V, TEST_PUBKEY, "9.9.9", false).is_err());
}

#[test]
fn a_signature_without_a_version_passes_only_when_not_required() {
    assert!(matches!(
        verify_signature(PAYLOAD, SIG_NO_VERSION, TEST_PUBKEY, "1.2.3", true),
        Err(Error::MissingSignedVersion)
    ));
    verify_signature(PAYLOAD, SIG_NO_VERSION, TEST_PUBKEY, "1.2.3", false).unwrap();
}

#[test]
fn rejects_garbage_keys_and_signatures() {
    for (sig, key) in [
        ("not base64!", TEST_PUBKEY),
        ("", TEST_PUBKEY),
        (SIG_V, "not base64!"),
        (SIG_V, ""),
        // base64 of text that is no minisign file
        ("aGVsbG8=", TEST_PUBKEY),
        (SIG_V, "aGVsbG8="),
    ] {
        assert!(
            matches!(
                verify_signature(PAYLOAD, sig, key, "1.2.3", false),
                Err(Error::Signature(_))
            ),
            "{sig:?} / {key:?}"
        );
    }
}

#[test]
fn the_production_key_is_the_tauri_apps() {
    use base64::Engine as _;
    let text = String::from_utf8(
        base64::engine::general_purpose::STANDARD
            .decode(PRODUCTION_PUBKEY)
            .unwrap(),
    )
    .unwrap();
    assert!(
        text.starts_with("untrusted comment: minisign public key: A427C18232B67439\n"),
        "{text}"
    );
    minisign_verify::PublicKey::decode(&text).unwrap();
    // Test builds are made without BOA_UPDATER_PUBKEY.
    assert!(pubkey_is_production());
    assert_eq!(embedded_pubkey(), PRODUCTION_PUBKEY);
}

#[test]
fn reads_the_signed_version() {
    assert_eq!(
        signed_version("timestamp:1794797536\tfile:Boa.app.tar.gz\tversion:0.2.1"),
        Some("0.2.1")
    );
    assert_eq!(signed_version("timestamp:1\tfile:x"), None);
    assert_eq!(signed_version(""), None);
}

/// A real Tauri update bundle and its .sig (e.g. the source repo's
/// `target/release/bundle/macos/`), checked against the production key.
/// Opt-in: `BOA_TEST_TAURI_BUNDLE_DIR=<dir> BOA_TEST_TAURI_BUNDLE_VERSION=<v>`.
#[test]
fn verifies_a_real_tauri_bundle_when_given_one() {
    let (Some(dir), Some(version)) = (
        std::env::var_os("BOA_TEST_TAURI_BUNDLE_DIR"),
        std::env::var("BOA_TEST_TAURI_BUNDLE_VERSION").ok(),
    ) else {
        return;
    };
    let dir = Path::new(&dir);
    let data = std::fs::read(dir.join("Boa.app.tar.gz")).unwrap();
    let sig = std::fs::read_to_string(dir.join("Boa.app.tar.gz.sig")).unwrap();
    verify_signature(&data, &sig, PRODUCTION_PUBKEY, &version, true).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    unpack(&data, &tmp.path().join("Boa.app")).unwrap();
}

// ── The feed over HTTP ───────────────────────────────────────────────────

/// Serves `routes` (path → (status, body)) on 127.0.0.1 until dropped.
pub(crate) struct Server {
    pub(crate) base: String,
}

pub(crate) fn serve(routes: Vec<(&'static str, u16, Vec<u8>)>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            if reader.read_line(&mut request).is_err() {
                continue;
            }
            // Skip the headers.
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                line.clear();
            }
            let path = request.split_whitespace().nth(1).unwrap_or("").to_owned();
            let (status, body) = routes
                .iter()
                .find(|(p, _, _)| *p == path)
                .map(|(_, s, b)| (*s, b.clone()))
                .unwrap_or((404, b"not found".to_vec()));
            let head = format!(
                "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
        }
    });
    Server { base }
}

pub(crate) fn manifest(base: &str, version: &str, file: &str, sig: &str) -> Vec<u8> {
    serde_json::json!({
        "version": version,
        "notes": "https://github.com/sharithg1/boa/releases/tag/v1.2.3",
        "pub_date": "2026-11-20T10:00:00Z",
        "platforms": {
            "darwin-aarch64": { "url": format!("{base}/{file}"), "signature": sig },
            "darwin-x86_64": { "url": format!("{base}/{file}"), "signature": sig },
        }
    })
    .to_string()
    .into_bytes()
}

#[test]
fn checks_the_feed_and_downloads_a_verified_update() {
    // Two servers: the downloads, and a feed whose manifests point at them.
    let files = serve(vec![
        ("/Boa.app.tar.gz", 200, PAYLOAD.to_vec()),
        (
            "/tampered.app.tar.gz",
            200,
            b"boa updater test payloaD\n".to_vec(),
        ),
    ]);
    let feed = serve(vec![
        (
            "/latest.json",
            200,
            manifest(&files.base, "1.2.3", "Boa.app.tar.gz", SIG_V),
        ),
        (
            "/tampered.json",
            200,
            manifest(&files.base, "1.2.3", "tampered.app.tar.gz", SIG_V),
        ),
        (
            "/lying.json",
            200,
            manifest(&files.base, "1.2.4", "Boa.app.tar.gz", SIG_V),
        ),
        ("/none.json", 204, Vec::new()),
        ("/html.json", 200, b"<html>rate limited</html>".to_vec()),
    ]);

    // latest.json → this Mac's download URL.
    let update = check(&format!("{}/latest.json", feed.base), "1.2.2", "aarch64")
        .unwrap()
        .unwrap();
    assert_eq!(update.version, "1.2.3");
    assert_eq!(update.url, format!("{}/Boa.app.tar.gz", files.base));
    assert_eq!(update.target, "darwin-aarch64");
    // Same version: nothing to do.
    assert!(
        check(&format!("{}/latest.json", feed.base), "1.2.3", "x86_64")
            .unwrap()
            .is_none()
    );
    // 204: nothing to do.
    assert!(
        check(&format!("{}/none.json", feed.base), "1.0.0", "aarch64")
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        check(&format!("{}/html.json", feed.base), "1.0.0", "aarch64"),
        Err(Error::Manifest(_))
    ));
    assert!(matches!(
        check(&format!("{}/missing.json", feed.base), "1.0.0", "aarch64"),
        Err(Error::Status { status: 404, .. })
    ));

    // Download + verify.
    let mut seen = Vec::new();
    let verified = download(&update, TEST_PUBKEY, |got, total| seen.push((got, total))).unwrap();
    assert_eq!(verified.version(), "1.2.3");
    assert_eq!(verified.bytes(), PAYLOAD);
    assert_eq!(
        seen.last(),
        Some(&(PAYLOAD.len() as u64, Some(PAYLOAD.len() as u64)))
    );
    // The production key does not accept the throwaway signature.
    assert!(matches!(
        download(&update, PRODUCTION_PUBKEY, |_, _| {}),
        Err(Error::Signature(_))
    ));

    // A tampered download is rejected.
    let tampered = check(&format!("{}/tampered.json", feed.base), "1.2.2", "aarch64")
        .unwrap()
        .unwrap();
    assert!(matches!(
        download(&tampered, TEST_PUBKEY, |_, _| {}),
        Err(Error::Signature(_))
    ));
    // A feed announcing another version than the one signed is rejected.
    let lying = check(&format!("{}/lying.json", feed.base), "1.2.2", "aarch64")
        .unwrap()
        .unwrap();
    assert!(matches!(
        download(&lying, TEST_PUBKEY, |_, _| {}),
        Err(Error::SignedVersion { .. })
    ));
}

#[test]
fn only_https_or_loopback_http() {
    for url in [
        "http://example.com/latest.json",
        "ftp://localhost/latest.json",
        "file:///tmp/latest.json",
    ] {
        assert!(
            matches!(check(url, "1.0.0", "aarch64"), Err(Error::InsecureUrl(_))),
            "{url}"
        );
    }
    for url in [
        "http://localhost:1/x",
        "http://127.0.0.1:1/x",
        "http://[::1]:1/x",
        "https://example.com/x",
    ] {
        assert!(fetch::checked_url(url).is_ok(), "{url}");
    }
    // A manifest on https pointing at a plain-http download fails at check time.
    let feed = serve(vec![(
        "/latest.json",
        200,
        manifest("http://example.com", "9.0.0", "Boa.app.tar.gz", SIG_V),
    )]);
    assert!(matches!(
        check(&format!("{}/latest.json", feed.base), "1.0.0", "aarch64"),
        Err(Error::InsecureUrl(_))
    ));
}

#[test]
fn fills_the_endpoint_variables() {
    assert_eq!(
        resolve_endpoint(
            "https://h/{{target}}/{{arch}}/{{current_version}}/{{bundle_type}}",
            "0.6.1",
            "aarch64"
        ),
        "https://h/darwin/aarch64/0.6.1/app"
    );
    assert_eq!(
        resolve_endpoint("https://h/%7B%7Barch%7D%7D", "0.6.1", "x86_64"),
        "https://h/x86_64"
    );
    assert_eq!(
        resolve_endpoint(UPDATE_ENDPOINT, "0.6.1", "x86_64"),
        UPDATE_ENDPOINT
    );
}

// ── Unpacking and installing ─────────────────────────────────────────────

/// A `.app.tar.gz` laid out as the Tauri bundler writes it: one `Boa.app/`
/// folder, an executable, Info.plist and a resource.
fn app_tar_gz(marker: &str) -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    let mut add = |path: &str, mode: u32, data: &[u8]| {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(mode);
        h.set_cksum();
        builder.append_data(&mut h, path, data).unwrap();
    };
    add(
        "Boa.app/Contents/Info.plist",
        0o644,
        format!("<plist><string>{marker}</string></plist>").as_bytes(),
    );
    add(
        "Boa.app/Contents/MacOS/boa-desktop",
        0o755,
        format!("#!/bin/sh\necho {marker}\n").as_bytes(),
    );
    add("Boa.app/Contents/Resources/icon.icns", 0o644, b"icns");
    builder.into_inner().unwrap().finish().unwrap()
}

fn fake_app(dir: &Path, marker: &str) -> std::path::PathBuf {
    let app = dir.join("Boa.app");
    std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
    std::fs::write(app.join("Contents/Info.plist"), marker).unwrap();
    std::fs::write(app.join("Contents/MacOS/boa-desktop"), marker).unwrap();
    std::fs::write(app.join("Contents/old-only"), "x").unwrap();
    app
}

#[test]
fn finds_the_bundle_of_an_executable() {
    assert_eq!(
        bundle_of_executable(Path::new(
            "/Applications/Boa.app/Contents/MacOS/boa-desktop"
        )),
        Some("/Applications/Boa.app".into())
    );
    assert_eq!(
        bundle_of_executable(Path::new("/x/target/release/boa-app")),
        None
    );
    assert_eq!(
        bundle_of_executable(Path::new("/x/Boa/Contents/MacOS/boa-desktop")),
        None
    );
}

#[test]
fn installs_by_swapping_the_bundle() {
    let tmp = tempfile::tempdir().unwrap();
    let apps = tmp.path().join("Applications");
    std::fs::create_dir(&apps).unwrap();
    let app = fake_app(&apps, "old");
    let update = VerifiedUpdate::unverified_for_tests("1.2.3", app_tar_gz("new"));
    install(&update, &app).unwrap();

    let plist = std::fs::read_to_string(app.join("Contents/Info.plist")).unwrap();
    assert_eq!(plist, "<plist><string>new</string></plist>");
    let exe = app.join("Contents/MacOS/boa-desktop");
    assert!(std::fs::read_to_string(&exe).unwrap().contains("echo new"));
    assert_eq!(exe.metadata().unwrap().permissions().mode() & 0o777, 0o755);
    assert!(app.join("Contents/Resources/icon.icns").is_file());
    // Nothing of the old bundle is left, in it or next to it.
    assert!(!app.join("Contents/old-only").exists());
    let siblings: Vec<_> = std::fs::read_dir(&apps)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(siblings, vec![std::ffi::OsString::from("Boa.app")]);
    // Readable by everyone, like any app bundle.
    assert_eq!(app.metadata().unwrap().permissions().mode() & 0o777, 0o755);
}

#[test]
fn a_bad_archive_leaves_the_bundle_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let app = fake_app(tmp.path(), "old");
    for bytes in [
        b"not a tarball".to_vec(),
        app_tar_gz("new")[..60].to_vec(),
        Vec::new(),
    ] {
        let update = VerifiedUpdate::unverified_for_tests("1.2.3", bytes);
        assert!(install(&update, &app).is_err());
        assert_eq!(
            std::fs::read_to_string(app.join("Contents/Info.plist")).unwrap(),
            "old"
        );
        assert!(app.join("Contents/old-only").exists());
    }
    let missing = tmp.path().join("Nope.app");
    let update = VerifiedUpdate::unverified_for_tests("1.2.3", app_tar_gz("new"));
    assert!(install(&update, &missing).is_err());
    assert!(!missing.exists());
}

#[test]
fn a_bundle_in_a_read_only_folder_is_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let apps = tmp.path().join("Applications");
    std::fs::create_dir(&apps).unwrap();
    let app = fake_app(&apps, "old");
    std::fs::set_permissions(&apps, std::fs::Permissions::from_mode(0o555)).unwrap();
    let update = VerifiedUpdate::unverified_for_tests("1.2.3", app_tar_gz("new"));
    let result = install(&update, &app);
    std::fs::set_permissions(&apps, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        matches!(&result, Err(Error::PermissionDenied(p)) if p == &app),
        "{result:?}"
    );
    assert_eq!(
        std::fs::read_to_string(app.join("Contents/Info.plist")).unwrap(),
        "old"
    );
}

/// A raw tar entry: path, type, link target, contents.
type RawEntry<'a> = (&'a str, tar::EntryType, &'a str, &'a [u8]);

/// Raw tar entries (the tar crate's Builder refuses unsafe paths).
fn raw_tar_gz(entries: &[RawEntry<'_>]) -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    for (path, kind, link, data) in entries {
        let mut h = tar::Header::new_gnu();
        let name = &mut h.as_gnu_mut().unwrap().name;
        name[..path.len()].copy_from_slice(path.as_bytes());
        h.set_entry_type(*kind);
        h.set_mode(0o755);
        h.set_size(data.len() as u64);
        if !link.is_empty() {
            let l = &mut h.as_gnu_mut().unwrap().linkname;
            l[..link.len()].copy_from_slice(link.as_bytes());
        }
        h.set_cksum();
        builder.append(&h, *data).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

#[test]
fn unpack_refuses_what_is_not_a_plain_bundle() {
    use tar::EntryType::{Regular, Symlink};
    let base: Vec<RawEntry<'_>> = vec![
        ("Boa.app/Contents/Info.plist", Regular, "", b"p"),
        ("Boa.app/Contents/MacOS/boa-desktop", Regular, "", b"x"),
    ];
    let ok = raw_tar_gz(&base);
    let tmp = tempfile::tempdir().unwrap();
    unpack(&ok, &tmp.path().join("ok.app")).unwrap();

    let cases: Vec<(&str, Vec<RawEntry<'_>>)> = vec![
        (
            "parent dir",
            vec![("Boa.app/../../evil", Regular, "", b"x")],
        ),
        ("absolute", vec![("/etc/evil", Regular, "", b"x")]),
        (
            "link out",
            vec![("Boa.app/Contents/Frameworks", Symlink, "../../../..", b"")],
        ),
        (
            "absolute link",
            vec![("Boa.app/Contents/x", Symlink, "/etc", b"")],
        ),
        (
            "two roots",
            vec![("Other.app/Contents/x", Regular, "", b"x")],
        ),
        (
            "device",
            vec![("Boa.app/Contents/dev", tar::EntryType::Char, "", b"")],
        ),
    ];
    for (i, (what, extra)) in cases.into_iter().enumerate() {
        let mut entries = base.clone();
        entries.extend(extra);
        let dest = tmp.path().join(format!("bad{i}.app"));
        assert!(
            matches!(unpack(&raw_tar_gz(&entries), &dest), Err(Error::Archive(_))),
            "{what}"
        );
    }
    // A link that stays inside the bundle is fine (frameworks have them).
    let mut entries = base.clone();
    entries.push((
        "Boa.app/Contents/Resources/link",
        Symlink,
        "../Info.plist",
        b"",
    ));
    unpack(&raw_tar_gz(&entries), &tmp.path().join("links.app")).unwrap();
    // No Info.plist: not a bundle.
    let only_exe = raw_tar_gz(&base[1..]);
    assert!(matches!(
        unpack(&only_exe, &tmp.path().join("noplist.app")),
        Err(Error::Archive(_))
    ));
    assert!(!tmp.path().join("etc").exists());
    assert!(!tmp.path().join("evil").exists());
}
