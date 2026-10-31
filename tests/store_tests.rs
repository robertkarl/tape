//! T2 acceptance tests: crypto + blob store invariants.
//!
//! These never touch the real transcript trees and never delete anything:
//! each test gets its own directory under /tmp and prints it.

use age::secrecy::ExposeSecret;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use tape::*;

fn tmpdir(tag: &str) -> PathBuf {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!("tape-test-{tag}-{}-{n}", std::process::id()));
    fs::create_dir_all(&p).unwrap();
    eprintln!("test dir: {}", p.display());
    p
}

fn sample() -> Vec<u8> {
    // Compressible, and long enough that zstd actually does something.
    let mut s = String::new();
    for i in 0..2000 {
        s.push_str(&format!(
            "{{\"type\":\"message\",\"text\":\"turn {i} hello world\"}}\n"
        ));
    }
    s.into_bytes()
}

#[test]
fn keygen_writes_0400_and_refuses_overwrite() {
    let d = tmpdir("keygen");
    let id = d.join("identity.age");
    let pk = d.join("pubkey.txt");

    store::keygen(&id, &pk).unwrap();

    let mode = fs::metadata(&id).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o400, "identity must be owner-read-only");
    assert!(fs::read_to_string(&pk).unwrap().contains("age1"));

    // Clobbering the identity must be impossible, not merely discouraged.
    assert!(store::keygen(&id, &pk).is_err());
    assert!(fs::read_to_string(&id).unwrap().contains("AGE-SECRET-KEY-"));
}

#[test]
fn roundtrip_is_byte_identical() {
    let d = tmpdir("roundtrip");
    let id = d.join("identity.age");
    let pk = d.join("pubkey.txt");
    let blobs = d.join("blobs");
    store::keygen(&id, &pk).unwrap();

    let plaintext = sample();
    let recipient = store::load_recipient(&pk).unwrap();
    let (blob, new) = store::store_bytes(&plaintext, &recipient, &blobs).unwrap();
    assert!(new);
    assert_eq!(blob.hash, blake3::hash(&plaintext).to_hex().to_string());

    let identity = store::load_identity(&id).unwrap();
    let back = store::verify_blob_with(&blob.path, &identity).unwrap();
    assert_eq!(back, plaintext.len() as u64);

    let sealed = store::seal(&plaintext, &recipient).unwrap();
    let recovered = store::open_and_check(&sealed, &identity, &blob.hash).unwrap();
    assert_eq!(recovered, plaintext);
}

#[test]
fn tampered_blob_fails() {
    let d = tmpdir("tamper");
    let id = d.join("identity.age");
    let pk = d.join("pubkey.txt");
    let blobs = d.join("blobs");
    store::keygen(&id, &pk).unwrap();

    let plaintext = sample();
    let recipient = store::load_recipient(&pk).unwrap();
    let (blob, _) = store::store_bytes(&plaintext, &recipient, &blobs).unwrap();

    // Flip one byte in the middle of the ciphertext.
    let mut bad = fs::read(&blob.path).unwrap();
    let mid = bad.len() / 2;
    bad[mid] ^= 0x01;
    let badpath = d.join("tampered.tape");
    fs::write(&badpath, &bad).unwrap();

    let identity = store::load_identity(&id).unwrap();
    let err = store::verify_blob_with(&badpath, &identity).unwrap_err();
    eprintln!("tamper detected: {err}");
}

#[test]
fn wrong_identity_fails() {
    let d = tmpdir("wrongkey");
    store::keygen(&d.join("identity.age"), &d.join("pubkey.txt")).unwrap();
    let blobs = d.join("blobs");
    let recipient = store::load_recipient(&d.join("pubkey.txt")).unwrap();
    let (blob, _) = store::store_bytes(&sample(), &recipient, &blobs).unwrap();

    let other = d.join("other.age");
    fs::write(
        &other,
        format!(
            "{}\n",
            age::x25519::Identity::generate()
                .to_string()
                .expose_secret()
        ),
    )
    .unwrap();
    let wrong = store::load_identity(&other).unwrap();
    assert!(store::verify_blob_with(&blob.path, &wrong).is_err());
}

#[test]
fn storing_twice_never_rewrites() {
    let d = tmpdir("idempotent");
    let id = d.join("identity.age");
    let pk = d.join("pubkey.txt");
    let blobs = d.join("blobs");
    store::keygen(&id, &pk).unwrap();
    let recipient = store::load_recipient(&pk).unwrap();

    let plaintext = sample();
    let (blob, first) = store::store_bytes(&plaintext, &recipient, &blobs).unwrap();
    assert!(first);
    let before = fs::read(&blob.path).unwrap();
    let mtime_before = fs::metadata(&blob.path).unwrap().modified().unwrap();

    let (again, second) = store::store_bytes(&plaintext, &recipient, &blobs).unwrap();
    assert!(!second, "a present blob must not be rewritten");
    assert_eq!(again.path, blob.path);
    let after = fs::read(&blob.path).unwrap();
    assert_eq!(before, after);
    assert_eq!(
        fs::metadata(&blob.path).unwrap().modified().unwrap(),
        mtime_before,
        "rewriting an existing blob would change its mtime"
    );
}

#[test]
fn atomic_write_leaves_no_temp_at_final_name() {
    let d = tmpdir("atomic");
    let target = d.join("deadbeef.tape");
    store::atomic_write(&target, b"payload").unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"payload");
    let leftovers: Vec<_> = fs::read_dir(&d)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".tmp-"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp files must be renamed away: {leftovers:?}"
    );
}

#[test]
fn store_all_rejects_files_that_changed_since_scan() {
    let d = tmpdir("drift");
    let id = d.join("identity.age");
    let pk = d.join("pubkey.txt");
    let blobs = d.join("blobs");
    store::keygen(&id, &pk).unwrap();

    let src = d.join("session.jsonl");
    fs::write(&src, b"original contents").unwrap();
    let meta = fs::metadata(&src).unwrap();
    let mut entry = tape::types::FileEntry {
        tool: tape::types::Tool::Claude,
        session_id: "s1".into(),
        group: "s1".into(),
        path: src.clone(),
        size: meta.len(),
        mtime_ms: store::now_ms(),
        blake3: "0".repeat(64), // deliberately wrong: simulates post-scan growth
        quiescent: true,
    };

    let report = store::store_all(std::slice::from_ref(&entry), &pk, &blobs, 1).unwrap();
    assert_eq!(report.stored, 0);
    assert_eq!(
        report.failures.len(),
        1,
        "drift must be reported, not stored"
    );
    eprintln!("drift detected: {}", report.failures[0]);

    // Fix the manifest entry and it stores cleanly.
    entry.blake3 = blake3::hash(&fs::read(&src).unwrap()).to_hex().to_string();
    let report = store::store_all(std::slice::from_ref(&entry), &pk, &blobs, 1).unwrap();
    assert_eq!(report.stored, 1);
    assert_eq!(report.failures.len(), 0);

    let v = store::verify_all(&blobs, &id, 1).unwrap();
    assert_eq!((v.checked, v.ok), (1, 1));
}

#[test]
fn atomic_write_uses_a_fresh_temp_and_never_touches_the_final_name_early() {
    let d = tmpdir("atomic2");
    let target = d.join("abcd.tape");
    // The exact temp name atomic_write will ask for in this process.
    let tmp = target.with_extension(format!("tape.tmp-{}", std::process::id()));
    fs::write(&tmp, b"half-written leftover").unwrap();

    let err = store::atomic_write(&target, b"payload");
    assert!(
        err.is_err(),
        "an existing temp orphan must not be silently clobbered"
    );
    assert!(
        !target.exists(),
        "the final name must not appear before the write completes"
    );
    assert_eq!(
        fs::read(&tmp).unwrap(),
        b"half-written leftover",
        "an orphaned temp from a killed run must be left exactly as it was"
    );
}

#[test]
fn non_quiescent_files_are_skipped_not_stored() {
    let d = tmpdir("quiescent");
    let id = d.join("identity.age");
    let pk = d.join("pubkey.txt");
    let blobs = d.join("blobs");
    store::keygen(&id, &pk).unwrap();

    let src = d.join("live.jsonl");
    fs::write(&src, b"still being written").unwrap();
    let entry = tape::types::FileEntry {
        tool: tape::types::Tool::Codex,
        session_id: "live".into(),
        group: "live".into(),
        path: src,
        size: 19,
        mtime_ms: store::now_ms(),
        blake3: "0".repeat(64),
        quiescent: false,
    };
    let report = store::store_all(&[entry], &pk, &blobs, 1).unwrap();
    assert_eq!(report.skipped_not_quiescent, 1);
    assert_eq!(report.stored, 0);
    assert_eq!(report.failures.len(), 0);
    assert_eq!(fs::read_dir(&blobs).unwrap().count(), 0);
}
