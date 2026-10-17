use super::*;
use crate::account::{AccountDraft, Provider, preset};
use crate::imap::MailboxEntry;
use crate::parse::parse;
use crate::store::Store;
use crate::store::tests::header;

/// The key every test that needs the process key uses (tests run in parallel and share it).
fn test_key() -> DbKey {
    DbKey::from_bytes(&[7u8; KEY_LEN]).unwrap()
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mail-cipher-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

/// A mailbox with a few messages, their bodies and an attachment (all in the database).
fn sample(dir: &Path) {
    let s = Store::open(&dir.join(DB_FILE)).unwrap();
    let account = s
        .add_account(&AccountDraft {
            provider: Provider::Imap,
            name: "Arbeit".into(),
            email: "maria@example.org".into(),
            sender_name: "Maria Muster".into(),
            transport: preset(Provider::Imap, "maria@example.org").unwrap(),
        })
        .unwrap();
    let inbox = s
        .upsert_mailbox(account, &MailboxEntry { name: "INBOX".into(), delimiter: Some('/'), attributes: vec![] })
        .unwrap();
    for i in 0..20u32 {
        let raw = format!(
            "From: Jonas <jonas@example.net>\r\nTo: maria@example.org\r\nSubject: Vertraulich {i}\r\nMessage-ID: <m{i}@example.net>\r\nDate: Mon, 6 Oct 2025 10:00:00 +0200\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nGeheimer Inhalt {i}\r\n--b\r\nContent-Type: text/plain; name=liste.txt\r\nContent-Disposition: attachment; filename=liste.txt\r\n\r\nAnhangsinhalt {i}\r\n--b--\r\n"
        );
        let p = parse(raw.as_bytes()).unwrap();
        let id = s.insert_message(inbox, &header(i + 1, raw.as_bytes()), &p).unwrap();
        s.set_body_raw(id, &p, Some(raw.as_bytes())).unwrap();
    }
}

fn subject(db: &Path, key: Option<&DbKey>, uid: u32) -> String {
    let c = open_with(db, OpenFlags::default(), key).unwrap();
    c.query_row("SELECT subject FROM messages WHERE uid = ?1", [uid], |r| r.get(0)).unwrap()
}

fn contains(path: &Path, needle: &[u8]) -> bool {
    let mut all = fs::read(path).unwrap_or_default();
    all.extend(fs::read(path.with_extension("db-wal")).unwrap_or_default());
    all.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn recovery_codes_round_trip_and_catch_typos() {
    let k = DbKey::generate().unwrap();
    let code = k.recovery_code();
    assert_eq!(code.len(), 14 * 4 + 13, "{code}");
    assert!(
        code.split('-').all(|g| g.len() == 4 && g.chars().all(|c| c.is_ascii_uppercase() || ('2'..='7').contains(&c)))
    );
    assert_eq!(DbKey::from_recovery_code(&code).unwrap(), k);
    assert_eq!(DbKey::from_recovery_code(&code.to_lowercase().replace('-', " ")).unwrap(), k);
    assert_eq!(DbKey::from_recovery_code(&code.replace('-', "")).unwrap(), k);
    let lookalike = code.replace('O', "0").replace('I', "1").replace('B', "8");
    assert_eq!(DbKey::from_recovery_code(&lookalike).unwrap(), k);
    let mut typo: Vec<char> = code.chars().collect();
    typo[5] = if typo[5] == 'A' { 'C' } else { 'A' };
    assert!(DbKey::from_recovery_code(&typo.into_iter().collect::<String>()).is_err());
    assert!(DbKey::from_recovery_code(&code[5..]).is_err());
    assert!(DbKey::from_recovery_code("").is_err());
    assert_eq!(DbKey::from_hex(&k.to_hex()).unwrap(), k);
    assert_eq!(format!("{k:?}"), "DbKey(…)");
}

#[test]
fn an_encrypted_copy_hides_bodies_and_attachments_and_needs_the_right_key() {
    let dir = tmp("access");
    sample(&dir);
    let db = dir.join(DB_FILE);
    let enc = dir.join("enc.db");
    let key = DbKey::generate().unwrap();
    assert_eq!(file_state(&db), FileState::Plain);
    assert!(contains(&db, b"Geheimer Inhalt") || contains(&db, b"Vertraulich"), "the sample is readable plain");
    export(&db, None, &enc, Some(&key)).unwrap();
    verify(&db, None, &enc, Some(&key)).unwrap();
    assert_eq!(file_state(&enc), FileState::Encrypted);
    for needle in [&b"Geheimer Inhalt"[..], b"Anhangsinhalt", b"Vertraulich", b"jonas@example.net"] {
        assert!(!contains(&enc, needle), "{} in the encrypted file", String::from_utf8_lossy(needle));
    }
    assert_eq!(access(&enc, None), Access::MissingKey);
    assert_eq!(access(&enc, Some(&key)), Access::Unlocked);
    assert_eq!(access(&enc, Some(&DbKey::generate().unwrap())), Access::WrongKey);
    let wrong = open_with(&enc, OpenFlags::default(), Some(&DbKey::generate().unwrap())).unwrap_err();
    assert_eq!(wrong.to_string(), wrong_key_text());
    assert_eq!(subject(&enc, Some(&key), 4), "Vertraulich 3");
    // The search index came along.
    let c = open_with(&enc, OpenFlags::default(), Some(&key)).unwrap();
    let hits: i64 = c
        .query_row("SELECT count(*) FROM messages_fts WHERE messages_fts MATCH 'vertraulich'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(hits, 20);
    drop(c);
    let back = dir.join("back.db");
    export(&enc, Some(&key), &back, None).unwrap();
    verify(&enc, Some(&key), &back, None).unwrap();
    assert_eq!(file_state(&back), FileState::Plain);
    assert_eq!(file_state(&dir.join("nope.db")), FileState::Missing);
    let junk = dir.join("junk.db");
    fs::write(&junk, "kein SQLite\n".repeat(300)).unwrap();
    assert_eq!(file_state(&junk), FileState::Plain);
    assert_eq!(access(&junk, None), Access::Plain);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn verify_finds_missing_rows() {
    let dir = tmp("verify");
    sample(&dir);
    let db = dir.join(DB_FILE);
    let copy = dir.join("copy.db");
    let key = DbKey::generate().unwrap();
    export(&db, None, &copy, Some(&key)).unwrap();
    {
        let c = open_with(&copy, OpenFlags::default(), Some(&key)).unwrap();
        c.execute("DELETE FROM attachments WHERE id = (SELECT max(id) FROM attachments)", []).unwrap();
    }
    let e = verify(&db, None, &copy, Some(&key)).unwrap_err().to_string();
    assert!(e.contains("attachments: "), "{e}");
    let plain = dir.join("plain.db");
    export(&db, None, &plain, None).unwrap();
    assert!(verify(&db, None, &plain, Some(&key)).is_err());
    let _ = fs::remove_dir_all(&dir);
}

/// Encrypts with a crash at `point` (if any), starts again, and checks the outcome.
fn encrypt_with_crash(point: Option<&str>) {
    let dir = tmp(&format!("kill-{}", point.unwrap_or("none")));
    sample(&dir);
    let key = test_key();
    request(&dir, Direction::Encrypt, 1).unwrap();
    if let Some(p) = point {
        let hook = |at: &str| at == p;
        assert!(run_pending_with(&dir, Some(&key), &hook).is_err(), "crash at {p}");
        let db = dir.join(DB_FILE);
        assert!(db.exists() || dir.join(NEW_FILE).exists(), "{p}: no database left");
    }
    let done = run_pending(&dir, Some(&key)).unwrap();
    assert_eq!(done, Some(Outcome { direction: Direction::Encrypt }), "{point:?}");
    assert_eq!(file_state(&dir.join(DB_FILE)), FileState::Encrypted);
    assert!(!dir.join(NEW_FILE).exists());
    assert_eq!(file_state(&dir.join(OLD_FILE)), FileState::Plain, "the original is kept");
    assert_eq!(subject(&dir.join(DB_FILE), Some(&key), 4), "Vertraulich 3");
    // Nothing more to do; the next start deletes the original.
    assert_eq!(run_pending(&dir, Some(&key)).unwrap(), None);
    assert!(old_left(&dir));
    assert!(confirm_open(&dir).unwrap());
    assert!(!old_left(&dir));
    assert!(read_marker(&dir).is_none());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn encrypting_survives_a_crash_at_every_step() {
    for p in [None, Some("exported"), Some("verified"), Some("old-moved"), Some("new-moved")] {
        encrypt_with_crash(p);
    }
}

#[test]
fn a_crash_while_exporting_leaves_the_original_alone() {
    let dir = tmp("partial");
    sample(&dir);
    request(&dir, Direction::Encrypt, 1).unwrap();
    fs::write(dir.join(NEW_FILE), b"half").unwrap();
    assert!(run_pending(&dir, Some(&test_key())).unwrap().is_some());
    assert_eq!(subject(&dir.join(DB_FILE), Some(&test_key()), 1), "Vertraulich 0");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_failed_check_rolls_back() {
    let dir = tmp("rollback");
    sample(&dir);
    request(&dir, Direction::Encrypt, 1).unwrap();
    assert!(request(&dir, Direction::Encrypt, 1).is_err(), "one switch at a time");
    assert!(run_pending(&dir, None).is_err());
    assert!(read_marker(&dir).is_none());
    assert_eq!(file_state(&dir.join(DB_FILE)), FileState::Plain);
    assert!(!dir.join(NEW_FILE).exists());
    assert!(request(&dir, Direction::Decrypt, 1).is_err());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn decrypting_works_the_same_way() {
    let dir = tmp("decrypt");
    sample(&dir);
    let key = test_key();
    request(&dir, Direction::Encrypt, 1).unwrap();
    run_pending(&dir, Some(&key)).unwrap();
    drop_old(&dir).unwrap();
    request(&dir, Direction::Decrypt, 2).unwrap();
    let hook = |at: &str| at == "old-moved";
    assert!(run_pending_with(&dir, Some(&key), &hook).is_err());
    assert_eq!(run_pending(&dir, Some(&key)).unwrap(), Some(Outcome { direction: Direction::Decrypt }));
    assert_eq!(file_state(&dir.join(DB_FILE)), FileState::Plain);
    assert_eq!(file_state(&dir.join(OLD_FILE)), FileState::Encrypted);
    assert_eq!(subject(&dir.join(DB_FILE), None, 20), "Vertraulich 19");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_switch_waits_for_open_connections_and_refuses_new_ones() {
    let dir = tmp("live");
    sample(&dir);
    let db = dir.join(DB_FILE);
    let key = DbKey::from_bytes(&[3u8; KEY_LEN]).unwrap();
    // A connection that stays open: the switch gives up, nothing changed.
    let held = Store::open(&db).unwrap();
    assert_eq!(open_count(&db), 1);
    let e = switch(&dir, Direction::Encrypt, &key, 1, Duration::from_millis(100)).unwrap_err();
    assert!(e.to_string().contains("Benutzung") || e.to_string().contains("in use"), "{e}");
    assert!(read_marker(&dir).is_none());
    assert_eq!(file_state(&db), FileState::Plain);
    // Closed while the switch waits: it goes ahead; a connection asked for meanwhile is refused.
    let path = db.clone();
    let closer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        let refused = Store::open(&path).is_err();
        drop(held);
        refused
    });
    let done = switch(&dir, Direction::Encrypt, &key, 2, Duration::from_secs(10)).unwrap();
    assert!(closer.join().unwrap(), "no new connection during the switch");
    assert_eq!(done.direction, Direction::Encrypt);
    assert_eq!(open_count(&db), 0);
    assert_eq!(file_state(&db), FileState::Encrypted);
    assert_eq!(subject(&db, Some(&key), 2), "Vertraulich 1");
    // The old plain file goes at the next start (or with „Jetzt sicher löschen“).
    assert!(contains(&dir.join(OLD_FILE), b"Vertraulich"));
    assert!(confirm_open(&dir).unwrap());
    assert!(!dir.join(OLD_FILE).exists());
    // And back while running.
    let done = switch(&dir, Direction::Decrypt, &key, 3, Duration::from_secs(1)).unwrap();
    assert_eq!(done.direction, Direction::Decrypt);
    assert_eq!(subject(&db, None, 3), "Vertraulich 2");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_store_opens_an_encrypted_file_with_the_process_key() {
    let dir = tmp("store-key");
    sample(&dir);
    request(&dir, Direction::Encrypt, 1).unwrap();
    run_pending(&dir, Some(&test_key())).unwrap();
    set_key(Some(test_key()));
    let s = Store::open(&dir.join(DB_FILE)).unwrap();
    let n: i64 = s.conn().query_row("SELECT count(*) FROM messages", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 20);
    drop(s);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_password_wraps_the_key() {
    let dir = tmp("wrap");
    let key = DbKey::generate().unwrap();
    assert!(WrappedKey::wrap(&key, "kurz", (1024, 1, 1)).is_err(), "at least 10 characters");
    let w = WrappedKey::wrap(&key, "Korrekt Pferd Batterie", (1024, 1, 1)).unwrap();
    w.write(&dir).unwrap();
    let back = WrappedKey::read(&dir).unwrap();
    assert_eq!(back.unwrap_key("Korrekt Pferd Batterie").unwrap(), key);
    assert!(back.unwrap_key("korrekt pferd batterie").is_err());
    let text = fs::read_to_string(dir.join(WRAPPED_FILE)).unwrap();
    assert!(!text.contains(key.to_hex().as_str()));
    WrappedKey::remove(&dir).unwrap();
    assert!(WrappedKey::read(&dir).is_none());
    let _ = fs::remove_dir_all(&dir);
}

/// A wrapped key from a data folder someone else made asks for an unbounded cost: refused at
/// once instead of tried.
#[test]
fn a_wrapped_key_with_an_excessive_cost_is_refused() {
    let key = DbKey::generate().unwrap();
    let w = WrappedKey::wrap(&key, "Ein Passwort", (8, 1, 1)).unwrap();
    for (m_kib, t, p) in [(8, 200_000, 1), (4 * 1024 * 1024, 1_000_000, 1), (8, 1, 64), (0, 1, 1)] {
        let costly = WrappedKey { m_kib, t, p, ..w.clone() };
        let start = Instant::now();
        let err = costly.unwrap_key("Ein Passwort").unwrap_err().to_string();
        assert!(start.elapsed() < Duration::from_secs(1), "{m_kib}/{t}/{p} took {:?}", start.elapsed());
        assert!(err.contains("Schlüsselformat") || err.contains("key format"), "{err}");
    }
    assert_eq!(w.unwrap_key("Ein Passwort").unwrap(), key);
}

#[test]
fn secure_delete_overwrites_then_removes() {
    let dir = tmp("shred");
    let f = dir.join("old.db");
    fs::write(&f, vec![0xAB; 200_000]).unwrap();
    secure_delete(&f).unwrap();
    assert!(!f.exists());
    secure_delete(&f).unwrap();
    let sub = dir.join("attachments").join("7");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("rechnung.pdf"), b"%PDF").unwrap();
    secure_delete_dir(&dir.join("attachments"));
    assert!(!dir.join("attachments").exists());
    let _ = fs::remove_dir_all(&dir);
}

/// SQLCipher may bundle an older SQLite than the plain build had: SQL that needs a newer one
/// fails at run time only. The functions added after 3.50 are not used anywhere.
#[test]
fn no_sql_newer_than_the_bundled_sqlite() {
    let version: String =
        Store::open_in_memory().unwrap().conn().query_row("SELECT sqlite_version()", [], |r| r.get(0)).unwrap();
    let minor: u32 = version.split('.').nth(1).and_then(|m| m.parse().ok()).unwrap_or(0);
    assert!(minor >= 50, "bundled SQLite {version}: check the list below against its release notes");
    const NEWER: [&str; 2] = ["json_array_insert", "jsonb_array_insert"];
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs" || x == "sql") {
                out.push(p);
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = vec![];
    walk(&root.join("src"), &mut files);
    walk(&root.join("migrations"), &mut files);
    walk(&root.join("../../src-tauri/src"), &mut files);
    for f in files.iter().filter(|f| !f.ends_with("cipher/tests.rs")) {
        let text = fs::read_to_string(f).unwrap_or_default().to_lowercase();
        for name in NEWER {
            assert!(!text.contains(&format!("{name}(")), "{} uses {name}", f.display());
        }
    }
}
