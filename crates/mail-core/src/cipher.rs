//! Encryption on the device, as in Arcalo: the database as a SQLCipher file (AES-256, a random
//! 256-bit key), switched on and off under Settings → Sicherheit. Off by default; a plain
//! database opens exactly as before (SQLCipher reads unencrypted files without a key).
//!
//! * Everything of the mail lives in the database: headers, bodies, the kept raw messages and
//!   their attachments, drafts and the offline queue, the search index. Encrypting the database
//!   covers all of it; the only mail on disk outside it are copies of attachments the user opened
//!   in another program, which the shell deletes at the start and when the app locks.
//! * The key: 32 random bytes, kept in the credential store (the shell's `secrets.rs`, entry
//!   [`KEY_ENTRY`]), handed to this module with [`set_key`] before the database is opened. It is
//!   applied as a raw key (`PRAGMA key = "x'…'"`), so there is no slow key derivation at start
//!   and the phones' background check opens the database as fast as before.
//! * Whether a file is encrypted is read from the file itself ([`file_state`]).
//! * The recovery key ([`DbKey::recovery_code`]) is the key in base32 with a short checksum, for a
//!   new device or a restored disk image where the credential is missing.
//! * Optionally the key is also kept in the data folder wrapped with a password ([`WrappedKey`]:
//!   Argon2id with a bounded cost and AES-256-GCM), for a portable copy that moves between
//!   computers.
//!
//! Switching ([`switch`]) runs while the app is open: the shell stops the account workers and
//! closes its own connection, the switch waits until no connection to the file is left
//! ([`open_count`]; new ones are refused meanwhile), exports the database with `sqlcipher_export`
//! into `mail.db.cipher-new`, checks it (`integrity_check`, the row count of every table, the
//! schema version) and only then swaps it in by two renames. Every step is written to
//! `cipher-migration.json`, so a crash at any point is either rolled back (before the swap the
//! original is untouched) or finished at the next start ([`run_pending`]). The previous file stays
//! as `mail.db.cipher-old` until the next start, then it is overwritten and deleted
//! ([`secure_delete`]; on SSDs and copy-on-write file systems overwriting is best effort).

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use crate::error::{Error, Result};
use crate::{tr, trf};

/// The database in the data folder.
pub const DB_FILE: &str = "mail.db";
/// Bytes of a database key (AES-256).
pub const KEY_LEN: usize = 32;
/// The credential store entry of the database key (hex).
pub const KEY_ENTRY: &str = "db-key";
/// The state of a switch between plain and encrypted (data folder).
pub const MARKER: &str = "cipher-migration.json";
/// The exported database before it is swapped in.
pub const NEW_FILE: &str = "mail.db.cipher-new";
/// The previous database after the swap, until the next start.
pub const OLD_FILE: &str = "mail.db.cipher-old";
/// The key wrapped with a password (optional, see [`WrappedKey`]).
pub const WRAPPED_FILE: &str = "db-key.wrapped.json";
/// The first bytes of every plain SQLite file.
const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";
/// Bytes of the checksum at the end of a recovery code.
const CHECK_LEN: usize = 3;

fn state_error(text: impl Into<String>) -> Error {
    Error::Invalid(text.into())
}

// ------------------------------------------------------------------------- key

/// A database key. Wiped from memory when dropped; never printed by `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct DbKey(Zeroizing<[u8; KEY_LEN]>);

impl std::fmt::Debug for DbKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DbKey(…)")
    }
}

impl DbKey {
    /// A new random key from the operating system's generator.
    pub fn generate() -> Result<Self> {
        let mut b = Zeroizing::new([0u8; KEY_LEN]);
        getrandom::fill(&mut b[..]).map_err(|e| state_error(format!("random: {e}")))?;
        Ok(DbKey(b))
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let arr: [u8; KEY_LEN] = bytes.try_into().ok()?;
        Some(DbKey(Zeroizing::new(arr)))
    }

    pub fn bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }

    /// Lower-case hex (how it is kept in the credential store).
    pub fn to_hex(&self) -> Zeroizing<String> {
        Zeroizing::new(self.0.iter().map(|b| format!("{b:02x}")).collect())
    }

    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.len() != KEY_LEN * 2 || !s.is_ascii() {
            return None;
        }
        let mut b = Zeroizing::new([0u8; KEY_LEN]);
        for (i, out) in b.iter_mut().enumerate() {
            *out = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
        }
        Some(DbKey(b))
    }

    /// The value of `PRAGMA key` / `ATTACH … KEY`: a raw key, no key derivation.
    fn sql(&self) -> Zeroizing<String> {
        Zeroizing::new(format!("x'{}'", self.to_hex().as_str()))
    }

    /// The recovery key: the key and a 3-byte checksum in base32 (A–Z, 2–7), 14 groups of four,
    /// e.g. `ABCD-EFGH-…`. Typing errors are found by the checksum, not taken as another key.
    pub fn recovery_code(&self) -> String {
        let mut raw = Zeroizing::new(Vec::with_capacity(KEY_LEN + CHECK_LEN));
        raw.extend_from_slice(&self.0[..]);
        raw.extend_from_slice(&checksum(&self.0)[..]);
        let text = Zeroizing::new(data_encoding::BASE32_NOPAD.encode(&raw));
        text.as_bytes().chunks(4).map(|c| std::str::from_utf8(c).unwrap_or_default()).collect::<Vec<_>>().join("-")
    }

    /// Reads a recovery key as typed: upper or lower case, with or without dashes and spaces;
    /// 0, 1 and 8 are read as O, I and B (they do not occur in the code).
    pub fn from_recovery_code(code: &str) -> Result<Self> {
        let mut clean = Zeroizing::new(String::with_capacity(64));
        for c in code.chars() {
            match c {
                '-' | ' ' | '\t' | '\n' | '\r' => {}
                '0' => clean.push('O'),
                '1' => clean.push('I'),
                '8' => clean.push('B'),
                c => clean.push(c.to_ascii_uppercase()),
            }
        }
        let invalid = || {
            Error::Invalid(
                tr!(
                    "Der Wiederherstellungsschlüssel ist unvollständig oder enthält einen Tippfehler.",
                    "The recovery key is incomplete or contains a typo."
                )
                .into(),
            )
        };
        let raw = Zeroizing::new(data_encoding::BASE32_NOPAD.decode(clean.as_bytes()).map_err(|_| invalid())?);
        if raw.len() != KEY_LEN + CHECK_LEN {
            return Err(invalid());
        }
        let key = DbKey::from_bytes(&raw[..KEY_LEN]).ok_or_else(invalid)?;
        if checksum(&key.0)[..] != raw[KEY_LEN..] {
            return Err(invalid());
        }
        Ok(key)
    }
}

fn checksum(key: &[u8; KEY_LEN]) -> [u8; CHECK_LEN] {
    let mut h = Sha256::new();
    h.update(b"arcalo-mail-recovery-key");
    h.update(key);
    let d = h.finalize();
    [d[0], d[1], d[2]]
}

/// The key of this process (`None`: no encrypted database, or the key is missing).
static KEY: RwLock<Option<DbKey>> = RwLock::new(None);

/// Sets the key every connection to an encrypted file uses (at the start, after the recovery
/// screen, after encrypting).
pub fn set_key(key: Option<DbKey>) {
    *KEY.write().unwrap_or_else(|e| e.into_inner()) = key;
}

pub fn key() -> Option<DbKey> {
    KEY.read().unwrap_or_else(|e| e.into_inner()).clone()
}

// ------------------------------------------------------------------- files

/// What a database file is, read from its first bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileState {
    /// No file, or an empty one (SQLite creates a plain database there).
    Missing,
    Plain,
    Encrypted,
}

pub fn file_state(path: &Path) -> FileState {
    let Ok(mut f) = fs::File::open(path) else { return FileState::Missing };
    let mut head = [0u8; 16];
    let mut n = 0;
    while n < head.len() {
        match f.read(&mut head[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(_) => return FileState::Missing,
        }
    }
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    match n {
        0 => FileState::Missing,
        16 if &head == SQLITE_HEADER => FileState::Plain,
        // An encrypted file is whole pages (4096 bytes by default, never below 512). Anything
        // else is a damaged or foreign file: counted as plain, so opening it fails as „not a
        // database“ instead of asking for a key.
        _ if len >= 512 && len % 512 == 0 => FileState::Encrypted,
        _ => FileState::Plain,
    }
}

/// The message when an encrypted database cannot be opened for lack of the key.
pub fn missing_key_text() -> String {
    tr!(
        "Die Datenbank ist verschlüsselt, der Schlüssel fehlt auf diesem Gerät.",
        "The database is encrypted and its key is missing on this device."
    )
    .into()
}

/// The message when the key does not open the database.
pub fn wrong_key_text() -> String {
    tr!("Der Schlüssel passt nicht zu dieser Datenbank.", "The key does not fit this database.").into()
}

fn switching_text() -> String {
    tr!(
        "Die Verschlüsselung wird gerade umgestellt. Versuch es gleich noch einmal.",
        "The encryption is being switched. Try again in a moment."
    )
    .into()
}

/// Applies `key` to a fresh connection and checks that it reads the file.
fn apply(conn: &Connection, key: &DbKey) -> Result<()> {
    conn.execute_batch(&format!("PRAGMA key = \"{}\";", key.sql().as_str()))?;
    match conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0)) {
        Ok(_) => Ok(()),
        Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::NotADatabase => {
            Err(state_error(wrong_key_text()))
        }
        Err(e) => Err(e.into()),
    }
}

/// Opens `path` with `flags`; an encrypted file gets `key` (or the key of this process).
pub fn open_with(path: &Path, flags: OpenFlags, key: Option<&DbKey>) -> Result<Connection> {
    let encrypted = file_state(path) == FileState::Encrypted;
    let conn = Connection::open_with_flags(path, flags)?;
    if encrypted {
        let own = if key.is_none() { self::key() } else { None };
        let key = key.or(own.as_ref()).ok_or_else(|| state_error(missing_key_text()))?;
        apply(&conn, key)?;
    }
    Ok(conn)
}

// -------------------------------------------------- connections of the app

/// Files being switched (no new connection) and how many connections each has open.
struct Gate {
    switching: Vec<PathBuf>,
    open: Vec<(PathBuf, usize)>,
}

static GATE: Mutex<Gate> = Mutex::new(Gate { switching: Vec::new(), open: Vec::new() });

fn gate() -> std::sync::MutexGuard<'static, Gate> {
    GATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// A connection of the app to a database file, counted while it lives (the switch waits for
/// every one to close).
#[derive(Debug)]
pub struct OpenFile(PathBuf);

impl Drop for OpenFile {
    fn drop(&mut self) {
        let mut g = gate();
        if let Some(e) = g.open.iter_mut().find(|(p, _)| *p == self.0) {
            e.1 = e.1.saturating_sub(1);
        }
        g.open.retain(|(_, n)| *n > 0);
    }
}

/// How the store opens its file: refused while the file is being switched, with the key of this
/// process when it is encrypted, counted until the returned [`OpenFile`] is dropped.
pub fn open_counted(path: &Path) -> Result<(Connection, OpenFile)> {
    {
        let mut g = gate();
        if g.switching.iter().any(|p| p == path) {
            return Err(state_error(switching_text()));
        }
        match g.open.iter_mut().find(|(p, _)| p == path) {
            Some(e) => e.1 += 1,
            None => g.open.push((path.to_path_buf(), 1)),
        }
    }
    let counted = OpenFile(path.to_path_buf());
    let conn = open_with(path, OpenFlags::default(), None)?;
    Ok((conn, counted))
}

/// Connections of the app open on `path`.
pub fn open_count(path: &Path) -> usize {
    gate().open.iter().find(|(p, _)| p == path).map_or(0, |(_, n)| *n)
}

/// How a database file can be opened (the start decides between the app and the recovery
/// screen with this).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    /// Plain or not there yet: no key needed.
    Plain,
    /// Encrypted, and the key opens it.
    Unlocked,
    /// Encrypted, no key.
    MissingKey,
    /// Encrypted, the key does not open it.
    WrongKey,
}

pub fn access(path: &Path, key: Option<&DbKey>) -> Access {
    if file_state(path) != FileState::Encrypted {
        return Access::Plain;
    }
    let Some(key) = key else { return Access::MissingKey };
    if key_opens(path, key) { Access::Unlocked } else { Access::WrongKey }
}

/// Whether `key` opens the encrypted file `path`.
pub fn key_opens(path: &Path, key: &DbKey) -> bool {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    Connection::open_with_flags(path, flags).is_ok_and(|c| apply(&c, key).is_ok())
}

/// Writes the database `src` (key `src_key`, `None` = plain) as a new file `dst` (key
/// `dst_key`), with `sqlcipher_export`. The schema version is carried over.
pub fn export(src: &Path, src_key: Option<&DbKey>, dst: &Path, dst_key: Option<&DbKey>) -> Result<()> {
    if dst.exists() {
        fs::remove_file(dst).map_err(|e| io_error(dst, e))?;
    }
    let conn = open_with(src, OpenFlags::default(), src_key)?;
    // Everything in the WAL is part of the source (the export reads through SQL anyway).
    let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
    let target = dst.to_str().ok_or_else(|| state_error(format!("{}", dst.display())))?;
    let key_sql = dst_key.map(|k| k.sql()).unwrap_or_else(|| Zeroizing::new(String::new()));
    conn.execute("ATTACH DATABASE ?1 AS cipher_target KEY ?2", [target, key_sql.as_str()])?;
    let res = (|| -> Result<()> {
        conn.query_row("SELECT sqlcipher_export('cipher_target')", [], |_| Ok(()))?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        conn.execute_batch(&format!("PRAGMA cipher_target.user_version = {version};"))?;
        Ok(())
    })();
    let _ = conn.execute_batch("DETACH DATABASE cipher_target;");
    res?;
    drop(conn);
    fsync(dst);
    Ok(())
}

fn io_error(path: &Path, e: std::io::Error) -> Error {
    Error::Invalid(format!("{}: {e}", path.display()))
}

/// Row count per table (the app's tables, not SQLite's own and not the search index's shadow
/// tables, which an export writes anew).
fn counts(conn: &Connection) -> Result<BTreeMap<String, i64>> {
    let names: Vec<String> = conn
        .prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
               AND sql NOT LIKE 'CREATE VIRTUAL TABLE%' ORDER BY name",
        )?
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    let mut out = BTreeMap::new();
    for n in names {
        let c: i64 =
            conn.query_row(&format!("SELECT count(*) FROM \"{}\"", n.replace('"', "\"\"")), [], |r| r.get(0))?;
        out.insert(n, c);
    }
    Ok(out)
}

/// Checks the copy `copy` against `orig`: `integrity_check` is `ok`, the same tables with the
/// same number of rows each, the same schema version, the expected encryption.
pub fn verify(orig: &Path, orig_key: Option<&DbKey>, copy: &Path, copy_key: Option<&DbKey>) -> Result<()> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let a = open_with(orig, flags, orig_key)?;
    let b = open_with(copy, flags, copy_key)?;
    if (copy_key.is_some()) != (file_state(copy) == FileState::Encrypted) {
        return Err(state_error(tr!("Die Kopie hat die falsche Verschlüsselung.", "The copy has the wrong encryption.")));
    }
    let check: String = b.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    if check != "ok" {
        return Err(state_error(trf!("Die Kopie ist beschädigt ({check}).", "The copy is damaged ({check}).")));
    }
    let va: i64 = a.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let vb: i64 = b.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if va != vb {
        return Err(state_error(trf!(
            "Die Kopie hat Schema-Version {vb} statt {va}.",
            "The copy has schema version {vb} instead of {va}."
        )));
    }
    let (ca, cb) = (counts(&a)?, counts(&b)?);
    if ca != cb {
        let diff: Vec<String> = ca
            .iter()
            .filter(|(k, v)| cb.get(*k) != Some(v))
            .map(|(k, v)| format!("{k}: {v} → {}", cb.get(k).map_or("–".into(), |c| c.to_string())))
            .collect();
        return Err(state_error(trf!(
            "Die Kopie hat nicht dieselben Zeilen ({}).",
            "The copy does not have the same rows ({}).",
            diff.join(", ")
        )));
    }
    Ok(())
}

fn fsync(path: &Path) {
    // Windows flushes only handles opened for writing.
    if let Ok(f) = fs::OpenOptions::new().write(true).open(path) {
        let _ = f.sync_all();
    }
}

pub fn fsync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(f) = fs::File::open(dir) {
        let _ = f.sync_all();
    }
    let _ = dir;
}

/// Writes `data` to `path` through a temporary file and a rename, synced: a crash never leaves
/// it cut short.
pub fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = path.with_extension("part");
    let res = (|| -> std::io::Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    })();
    res.map_err(|e| io_error(path, e))?;
    if let Some(dir) = path.parent() {
        fsync_dir(dir);
    }
    Ok(())
}

/// Overwrites `path` with zeros, syncs and deletes it. Best effort: SSDs (wear levelling),
/// copy-on-write file systems (APFS, Btrfs), snapshots and backups of the disk may keep the old
/// blocks; full-disk encryption (BitLocker, FileVault, LUKS) covers those.
pub fn secure_delete(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let res = (|| -> std::io::Result<()> {
        let len = fs::metadata(path)?.len();
        let mut f = fs::OpenOptions::new().write(true).open(path)?;
        let zeros = vec![0u8; 64 * 1024];
        let mut left = len;
        while left > 0 {
            let n = left.min(zeros.len() as u64) as usize;
            f.write_all(&zeros[..n])?;
            left -= n as u64;
        }
        f.sync_all()
    })();
    // Not overwritten (a read-only file, a full disk): it is deleted all the same.
    let _ = res;
    fs::remove_file(path).map_err(|e| io_error(path, e))
}

/// Overwrites and deletes every file below `dir` and the folder itself (the copies of opened
/// attachments).
pub fn secure_delete_dir(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            secure_delete_dir(&p);
        } else {
            let _ = secure_delete(&p);
        }
    }
    let _ = fs::remove_dir(dir);
}

// ----------------------------------------------------------------- switching

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Encrypt,
    Decrypt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// Asked for; nothing done yet.
    Requested,
    /// The new file is written and checked; the swap may begin.
    Verified,
    /// The new file is the database; the previous one is `OLD_FILE`.
    Swapped,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Migration {
    pub direction: Direction,
    pub step: Step,
    /// Unix seconds.
    pub requested: i64,
}

pub fn read_marker(dir: &Path) -> Option<Migration> {
    serde_json::from_slice(&fs::read(dir.join(MARKER)).ok()?).ok()
}

fn write_marker(dir: &Path, m: &Migration) -> Result<()> {
    write_atomic(&dir.join(MARKER), &serde_json::to_vec_pretty(m)?)
}

fn remove_marker(dir: &Path) {
    let _ = fs::remove_file(dir.join(MARKER));
    fsync_dir(dir);
}

/// Asks for a switch. Refused while one is unfinished or the database already is what is asked
/// for.
pub fn request(dir: &Path, direction: Direction, now: i64) -> Result<()> {
    if let Some(m) = read_marker(dir)
        && m.step != Step::Swapped
    {
        return Err(state_error(tr!(
            "Eine Umstellung der Verschlüsselung läuft bereits.",
            "Switching the encryption is already in progress."
        )));
    }
    let state = file_state(&dir.join(DB_FILE));
    let already = match direction {
        Direction::Encrypt => state == FileState::Encrypted,
        Direction::Decrypt => state != FileState::Encrypted,
    };
    if already {
        return Err(state_error(match direction {
            Direction::Encrypt => tr!("Die Datenbank ist schon verschlüsselt.", "The database is already encrypted."),
            Direction::Decrypt => tr!("Die Datenbank ist nicht verschlüsselt.", "The database is not encrypted."),
        }));
    }
    // The previous file of an earlier switch goes now (it is replaced by this one's).
    drop_old(dir)?;
    write_marker(dir, &Migration { direction, step: Step::Requested, requested: now })
}

/// Drops a request that has not started.
pub fn cancel_request(dir: &Path) -> bool {
    match read_marker(dir) {
        Some(m) if m.step == Step::Requested => {
            remove_marker(dir);
            true
        }
        _ => false,
    }
}

/// What a switch did.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Outcome {
    pub direction: Direction,
}

/// Points in [`run_pending_with`] where the tests stop it as a crash would.
pub type KillHook<'a> = &'a dyn Fn(&str) -> bool;

/// Carries out (or finishes) a requested switch while no connection is open on the database.
/// `key`: the database key (needed in both directions). `Ok(Some)` when the switch finished now;
/// `Err` when it was rolled back (the database is unchanged) or must be finished at the next
/// start (a rename the system refused).
pub fn run_pending(dir: &Path, key: Option<&DbKey>) -> Result<Option<Outcome>> {
    run_pending_with(dir, key, &|_| false)
}

pub fn run_pending_with(dir: &Path, key: Option<&DbKey>, kill: KillHook) -> Result<Option<Outcome>> {
    let Some(mut m) = read_marker(dir) else { return Ok(None) };
    let (db, new, old) = (dir.join(DB_FILE), dir.join(NEW_FILE), dir.join(OLD_FILE));
    let crashed = || Err(state_error("killed (test)"));
    if m.step == Step::Requested {
        let rollback = |e: Error| {
            let _ = fs::remove_file(&new);
            remove_marker(dir);
            Err(e)
        };
        let Some(key) = key else {
            return rollback(state_error(missing_key_text()));
        };
        let (src_key, dst_key) = match m.direction {
            Direction::Encrypt => (None, Some(key)),
            Direction::Decrypt => (Some(key), None),
        };
        let wanted = match m.direction {
            Direction::Encrypt => FileState::Plain,
            Direction::Decrypt => FileState::Encrypted,
        };
        // Already switched (a second request, a file put back by hand).
        if file_state(&db) != wanted {
            remove_marker(dir);
            return Ok(None);
        }
        if let Err(e) = export(&db, src_key, &new, dst_key) {
            return rollback(e);
        }
        if kill("exported") {
            return crashed();
        }
        if let Err(e) = verify(&db, src_key, &new, dst_key) {
            return rollback(e);
        }
        m.step = Step::Verified;
        write_marker(dir, &m)?;
        if kill("verified") {
            return crashed();
        }
    }
    if m.step == Step::Verified {
        let unfinished = |e: std::io::Error| {
            state_error(trf!(
                "Die Umstellung wird beim nächsten Start abgeschlossen ({e}).",
                "The switch is finished at the next start ({e})."
            ))
        };
        // Each rename is checked against the files present, so a crash between them resumes.
        if new.exists() && db.exists() && !old.exists() {
            let _ = fs::remove_file(dir.join(format!("{DB_FILE}-shm")));
            fs::rename(&db, &old).map_err(unfinished)?;
            let wal = dir.join(format!("{DB_FILE}-wal"));
            if wal.exists() {
                fs::rename(&wal, dir.join(format!("{OLD_FILE}-wal"))).map_err(unfinished)?;
            }
            fsync_dir(dir);
        }
        if kill("old-moved") {
            return crashed();
        }
        if new.exists() && !db.exists() {
            fs::rename(&new, &db).map_err(unfinished)?;
            fsync_dir(dir);
        }
        if kill("new-moved") {
            return crashed();
        }
        if new.exists() {
            // Both files present and no previous one: the marker says verified but the export is
            // from an earlier attempt. Start over.
            let _ = fs::remove_file(&new);
            m.step = Step::Requested;
            write_marker(dir, &m)?;
            return Err(state_error(tr!(
                "Umstellung unterbrochen, sie wird wiederholt.",
                "Switch interrupted; it will be repeated."
            )));
        }
        m.step = Step::Swapped;
        write_marker(dir, &m)?;
        return Ok(Some(Outcome { direction: m.direction }));
    }
    Ok(None)
}

/// The switch while the app runs: refuses new connections to the database, waits up to `wait`
/// until the app's connections are closed (the shell stops its workers and drops its own one
/// first), then runs the switch. Too long a wait drops the request; nothing changed then.
pub fn switch(dir: &Path, direction: Direction, key: &DbKey, now: i64, wait: Duration) -> Result<Outcome> {
    switch_with(dir, direction, key, now, wait, &|_| false)
}

pub fn switch_with(
    dir: &Path,
    direction: Direction,
    key: &DbKey,
    now: i64,
    wait: Duration,
    kill: KillHook,
) -> Result<Outcome> {
    let db = dir.join(DB_FILE);
    request(dir, direction, now)?;
    gate().switching.push(db.clone());
    let release = || gate().switching.retain(|p| *p != db);
    let start = Instant::now();
    while open_count(&db) > 0 {
        if start.elapsed() >= wait {
            cancel_request(dir);
            release();
            return Err(state_error(tr!(
                "Die Datenbank ist noch in Benutzung. Versuch es gleich noch einmal.",
                "The database is still in use. Try again in a moment."
            )));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let res = run_pending_with(dir, Some(key), kill);
    release();
    match res? {
        Some(o) => Ok(o),
        None => Err(state_error(match direction {
            Direction::Encrypt => tr!("Die Datenbank ist schon verschlüsselt.", "The database is already encrypted."),
            Direction::Decrypt => tr!("Die Datenbank ist nicht verschlüsselt.", "The database is not encrypted."),
        })),
    }
}

/// The database opened after a switch: the previous file of a switch finished before this start
/// is overwritten and deleted. Returns whether it was.
pub fn confirm_open(dir: &Path) -> Result<bool> {
    match read_marker(dir) {
        Some(m) if m.step == Step::Swapped => {
            drop_old(dir)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Overwrites and deletes the previous file of a finished switch now.
pub fn drop_old(dir: &Path) -> Result<()> {
    secure_delete(&dir.join(OLD_FILE))?;
    secure_delete(&dir.join(format!("{OLD_FILE}-wal")))?;
    let _ = fs::remove_file(dir.join(format!("{OLD_FILE}-shm")));
    if read_marker(dir).is_some_and(|m| m.step == Step::Swapped) {
        remove_marker(dir);
    }
    Ok(())
}

/// The previous file of a finished switch is still there.
pub fn old_left(dir: &Path) -> bool {
    dir.join(OLD_FILE).exists()
}

// ------------------------------------------------------------ wrapped key

/// The database key wrapped with a password: Argon2id derives a key from the password, AES-256-GCM
/// encrypts the database key with it. Kept in the data folder (it travels with a portable copy);
/// without the password it is useless.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WrappedKey {
    pub v: u32,
    pub kdf: String,
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
    pub salt: String,
    pub nonce: String,
    pub ct: String,
}

/// Argon2id cost of the password: 64 MiB, 3 passes (about half a second on a laptop).
pub const WRAP_COST: (u32, u32, u32) = (64 * 1024, 3, 1);
/// The highest cost a wrapped key may ask for (1 GiB, 10 passes, 4 lanes): a file from someone
/// else must not ask for gigabytes of memory or hours of passes when the password is typed.
pub const MAX_WRAP_COST: (u32, u32, u32) = (1024 * 1024, 10, 4);
/// The shortest password for the wrapped key.
pub const MIN_PASSWORD: usize = 10;

fn kek(password: &str, salt: &[u8], (m, t, p): (u32, u32, u32)) -> Result<Zeroizing<[u8; 32]>> {
    let params = argon2::Params::new(m, t, p, Some(32)).map_err(|e| state_error(e.to_string()))?;
    let a = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut out = Zeroizing::new([0u8; 32]);
    a.hash_password_into(password.as_bytes(), salt, &mut out[..]).map_err(|e| state_error(e.to_string()))?;
    Ok(out)
}

fn wrong_password() -> Error {
    state_error(tr!("Das Passwort ist falsch.", "The password is wrong."))
}

impl WrappedKey {
    pub fn wrap(key: &DbKey, password: &str, cost: (u32, u32, u32)) -> Result<Self> {
        use aes_gcm::aead::{Aead, KeyInit};
        if password.chars().count() < MIN_PASSWORD {
            return Err(Error::Invalid(
                tr!("Das Passwort braucht mindestens 10 Zeichen.", "The password needs at least 10 characters.").into(),
            ));
        }
        let b64 = |b: &[u8]| data_encoding::BASE64.encode(b);
        let mut salt = [0u8; 16];
        let mut nonce = [0u8; 12];
        getrandom::fill(&mut salt).and_then(|_| getrandom::fill(&mut nonce)).map_err(|e| state_error(e.to_string()))?;
        let k = kek(password, &salt, cost)?;
        let cipher = aes_gcm::Aes256Gcm::new_from_slice(&k[..]).map_err(|e| state_error(e.to_string()))?;
        let ct = cipher
            .encrypt(aes_gcm::Nonce::from_slice(&nonce), &key.bytes()[..])
            .map_err(|_| state_error("encrypt"))?;
        Ok(WrappedKey {
            v: 1,
            kdf: "argon2id".into(),
            m_kib: cost.0,
            t: cost.1,
            p: cost.2,
            salt: b64(&salt),
            nonce: b64(&nonce),
            ct: b64(&ct),
        })
    }

    pub fn unwrap_key(&self, password: &str) -> Result<DbKey> {
        use aes_gcm::aead::{Aead, KeyInit};
        let d = |s: &str| data_encoding::BASE64.decode(s.as_bytes()).map_err(|_| wrong_password());
        let (m, t, p) = (self.m_kib, self.t, self.p);
        let bounded = (8..=MAX_WRAP_COST.0).contains(&m) && (1..=MAX_WRAP_COST.1).contains(&t) && (1..=MAX_WRAP_COST.2).contains(&p);
        if self.v != 1 || self.kdf != "argon2id" || !bounded {
            return Err(state_error(tr!("Unbekanntes Schlüsselformat.", "Unknown key format.")));
        }
        let (salt, nonce, ct) = (d(&self.salt)?, d(&self.nonce)?, d(&self.ct)?);
        if nonce.len() != 12 || salt.len() < 8 {
            return Err(wrong_password());
        }
        let k = kek(password, &salt, (m, t, p))?;
        let cipher = aes_gcm::Aes256Gcm::new_from_slice(&k[..]).map_err(|_| wrong_password())?;
        let mut plain = cipher.decrypt(aes_gcm::Nonce::from_slice(&nonce), ct.as_ref()).map_err(|_| wrong_password())?;
        let key = DbKey::from_bytes(&plain).ok_or_else(wrong_password);
        plain.zeroize();
        key
    }

    pub fn read(dir: &Path) -> Option<Self> {
        serde_json::from_slice(&fs::read(dir.join(WRAPPED_FILE)).ok()?).ok()
    }

    /// Written to a temporary file and renamed: a full disk or a crash never leaves the only
    /// password-wrapped copy of the key cut short.
    pub fn write(&self, dir: &Path) -> Result<()> {
        write_atomic(&dir.join(WRAPPED_FILE), &serde_json::to_vec_pretty(self)?)
    }

    pub fn remove(dir: &Path) -> Result<()> {
        let path = dir.join(WRAPPED_FILE);
        if path.exists() {
            fs::remove_file(&path).map_err(|e| io_error(&path, e))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
