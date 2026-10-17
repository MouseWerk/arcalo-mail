//! The update feed and its files: `latest.json` as the release workflow writes it, the entry of
//! this platform, a download that continues after an interruption or a pause, and the signature
//! check (as Arcalo's `update_feed`, with the GitHub release as the only source).
//!
//! Every file is checked against the public key compiled into the app: whoever serves the files
//! cannot change them. The signature's trusted comment names the version (the Tauri CLI signs it
//! so), so a feed cannot pair a new version number with an older file.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::network::HttpClient;
use crate::{tr, trf};

/// `latest.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub pub_date: Option<String>,
    #[serde(default, deserialize_with = "readable_platforms")]
    pub platforms: BTreeMap<String, Platform>,
}

/// The platform entries this version can read: one it cannot (a platform added later with other
/// fields) is left out instead of making the feed unreadable for every platform.
fn readable_platforms<'de, D: serde::Deserializer<'de>>(d: D) -> Result<BTreeMap<String, Platform>, D::Error> {
    let raw: Option<BTreeMap<String, serde_json::Value>> = Option::deserialize(d)?;
    Ok(raw.unwrap_or_default().into_iter().filter_map(|(k, v)| Some((k, serde_json::from_value(v).ok()?))).collect())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Platform {
    pub url: String,
    pub signature: String,
}

/// The platform keys this copy accepts, most specific first (`windows-x86_64-nsis`, then
/// `windows-x86_64`).
pub fn targets(os_arch: &str, installer: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(i) = installer.filter(|i| !i.is_empty()) {
        out.push(format!("{os_arch}-{i}"));
    }
    out.push(os_arch.to_string());
    out
}

/// The entry of `manifest` for the first of `targets` it has.
pub fn platform<'a>(manifest: &'a Manifest, targets: &[String]) -> Option<&'a Platform> {
    targets.iter().find_map(|t| manifest.platforms.get(t))
}

/// Why the feed gave no usable answer, or a download failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedError {
    /// No feed there (HTTP 404: no release yet, or one without this file).
    Missing,
    Connect,
    Timeout,
    /// The connection broke off midway.
    Interrupted,
    /// The server answered with another error status.
    Status(u16),
    /// The feed could not be read.
    Invalid(String),
    /// The download could not be written (the message names the file).
    Io(String),
    /// The disk is full (the download stopped; its part stays).
    DiskFull,
    /// The signature does not match the file or the key.
    Signature(String),
    /// No file for this system in that version.
    NoPlatform,
    /// A download address the app does not take (only HTTPS, or this computer in tests).
    Refused(String),
}

impl FeedError {
    pub fn message(&self) -> String {
        match self {
            FeedError::Missing => tr!(
                "Der Update-Server hat keine Versionsinformation geliefert",
                "The update server sent no version information"
            )
            .into(),
            FeedError::Connect => tr!(
                "Keine Verbindung zum Update-Server (offline, oder den Proxy unter Einstellungen → Netzwerk prüfen)",
                "No connection to the update server (offline, or check the proxy under Settings → Network)"
            )
            .into(),
            FeedError::Timeout => tr!(
                "Zeitüberschreitung – der Update-Server antwortet nicht",
                "Timed out – the update server does not answer"
            )
            .into(),
            FeedError::Interrupted => tr!("Die Verbindung wurde unterbrochen", "The connection was interrupted").into(),
            FeedError::Status(code) => trf!(
                "Der Server hat die Datei nicht geliefert (Status {code})",
                "The server did not deliver the file (status {code})"
            ),
            FeedError::Invalid(_) => {
                tr!("Die Versionsinformation ist ungültig", "The version information is invalid").into()
            }
            FeedError::Io(e) => e.clone(),
            FeedError::DiskFull => tr!(
                "Nicht genug freier Speicherplatz für das Update – mach Platz frei, dann geht der Download dort weiter, wo er stehen geblieben ist",
                "Not enough free disk space for the update – free some space, then the download continues where it stopped"
            )
            .into(),
            FeedError::Signature(_) => tr!(
                "Die Signatur des Updates ist ungültig – die Datei wurde verworfen, es wurde nichts installiert",
                "The update's signature is invalid – the file was discarded, nothing was installed"
            )
            .into(),
            FeedError::NoPlatform => tr!(
                "Für dieses System gibt es in dieser Version kein Update-Paket",
                "This version has no update package for this system"
            )
            .into(),
            FeedError::Refused(url) => trf!(
                "Die Download-Adresse wird nicht angenommen: {url}",
                "The download address is not accepted: {url}"
            ),
        }
    }

    pub fn of_reqwest(e: &reqwest::Error) -> FeedError {
        if e.is_timeout() {
            FeedError::Timeout
        } else if e.is_connect() {
            FeedError::Connect
        } else if e.is_body() || e.is_decode() || e.is_request() {
            FeedError::Interrupted
        } else {
            FeedError::Io(e.to_string())
        }
    }

    /// Writing the download (its folder, the part file) failed: a full disk is told as such.
    fn of_write(path: &Path, e: &std::io::Error) -> FeedError {
        if matches!(e.kind(), std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded) {
            return FeedError::DiskFull;
        }
        FeedError::Io(trf!(
            "{path} kann nicht geschrieben werden ({e})",
            "{path} cannot be written ({e})",
            path = path.display()
        ))
    }
}

/// Reads `latest.json` from `url`.
pub async fn fetch(client: &HttpClient, url: &str) -> Result<Manifest, FeedError> {
    let text = get_text(client, url).await?;
    parse(&text)
}

/// The text at `url` (the desktop's and the phones' feed); 404 is [`FeedError::Missing`].
pub async fn get_text(client: &HttpClient, url: &str) -> Result<String, FeedError> {
    crate::network::check_https_url(url).map_err(|_| FeedError::Refused(url.to_string()))?;
    let res = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| FeedError::of_reqwest(&e))?;
    match res.status().as_u16() {
        200..=299 => {}
        404 => return Err(FeedError::Missing),
        code => return Err(FeedError::Status(code)),
    }
    res.text().await.map_err(|e| FeedError::of_reqwest(&e))
}

/// A feed with a version that is not `MAJOR.MINOR.PATCH` is invalid.
pub fn parse(text: &str) -> Result<Manifest, FeedError> {
    let manifest: Manifest =
        serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| FeedError::Invalid(e.to_string()))?;
    if semver::Version::parse(manifest.version.trim().trim_start_matches('v')).is_err() {
        return Err(FeedError::Invalid(format!("version {}", manifest.version)));
    }
    Ok(manifest)
}

/// How a download ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Done,
    /// Stopped on request; the part file stays for [`download`] to continue.
    Paused,
}

/// Downloads `url` into `part`, continuing an earlier part (HTTP `Range`). `pause` stops it
/// between chunks; `progress(done, total)` reports each chunk.
pub async fn download(
    client: &HttpClient,
    url: &str,
    part: &Path,
    pause: &AtomicBool,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<Step, FeedError> {
    crate::network::check_https_url(url).map_err(|_| FeedError::Refused(url.to_string()))?;
    if let Some(dir) = part.parent() {
        std::fs::create_dir_all(dir).map_err(|e| FeedError::of_write(dir, &e))?;
    }
    let mut have = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    let open = |append: bool| {
        std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(part)
            .map_err(|e| FeedError::of_write(part, &e))
    };
    loop {
        let mut req = client.get(url).header(reqwest::header::ACCEPT, "application/octet-stream");
        if have > 0 {
            req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let mut res = req.send().await.map_err(|e| FeedError::of_reqwest(&e))?;
        let status = res.status().as_u16();
        // The part was complete already.
        if status == 416 && have > 0 {
            progress(have, Some(have));
            return Ok(Step::Done);
        }
        if !(200..300).contains(&status) {
            return Err(FeedError::Status(status));
        }
        // 206 has to continue exactly where the part ends: a server or proxy answering another
        // range would splice two pieces of the file. Then the whole file is asked for instead.
        if status == 206
            && let Some(start) = content_range_start(res.headers())
            && start != have
        {
            if have == 0 {
                return Err(FeedError::Interrupted);
            }
            have = 0;
            continue;
        }
        // 206: the server continues where the part ends; 200: it sends the whole file again.
        let resumed = status == 206 && have > 0;
        let mut done = if resumed { have } else { 0 };
        let total = res.content_length().map(|l| l + done);
        let mut file = open(resumed)?;
        progress(done, total);
        while let Some(chunk) = res.chunk().await.map_err(|e| FeedError::of_reqwest(&e))? {
            file.write_all(&chunk).map_err(|e| FeedError::of_write(part, &e))?;
            done += chunk.len() as u64;
            progress(done, total);
            if pause.load(Ordering::SeqCst) {
                let _ = file.flush();
                return Ok(Step::Paused);
            }
        }
        if total.is_some_and(|t| done < t) {
            return Err(FeedError::Interrupted);
        }
        file.flush().map_err(|e| FeedError::of_write(part, &e))?;
        return Ok(Step::Done);
    }
}

/// The first byte of a `Content-Range: bytes <first>-<last>/<size>` answer.
fn content_range_start(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let value = headers.get(reqwest::header::CONTENT_RANGE)?.to_str().ok()?;
    value.trim().strip_prefix("bytes ")?.split('-').next()?.trim().parse().ok()
}

/// Checks `data` against the minisign `signature` (base64 of the `.sig` file, as `latest.json`
/// carries it) with `pubkey` (base64 of the public key file), and that the signed trusted comment
/// names `version`.
pub fn verify(data: &[u8], signature: &str, pubkey: &str, version: &str) -> Result<(), FeedError> {
    let (key, sig) = decode_signature(signature, pubkey)?;
    key.verify(data, &sig, true).map_err(|e| FeedError::Signature(e.to_string()))?;
    signed_version(&sig, version)
}

fn decode_signature(
    signature: &str,
    pubkey: &str,
) -> Result<(minisign_verify::PublicKey, minisign_verify::Signature), FeedError> {
    let b64 = |s: &str| {
        base64::engine::general_purpose::STANDARD
            .decode(s.trim())
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .ok_or_else(|| FeedError::Signature("not base64".into()))
    };
    let key = minisign_verify::PublicKey::decode(&b64(pubkey)?).map_err(|e| FeedError::Signature(e.to_string()))?;
    let sig = minisign_verify::Signature::decode(&b64(signature)?).map_err(|e| FeedError::Signature(e.to_string()))?;
    Ok((key, sig))
}

/// The version named in the signature's trusted comment must be `version`.
fn signed_version(sig: &minisign_verify::Signature, version: &str) -> Result<(), FeedError> {
    let signed = sig.trusted_comment().split('\t').find_map(|f| f.strip_prefix("version:"));
    let p = |v: &str| semver::Version::parse(v.trim().trim_start_matches('v')).ok();
    match signed {
        Some(s) if p(s).is_some() && p(s) == p(version) => Ok(()),
        Some(s) => Err(FeedError::Signature(format!("signed for {s}, announced {version}"))),
        None => Err(FeedError::Signature("no signed version".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // A throwaway key and the signature of PAYLOAD for version 1.9.1, the test vector of Arcalo's
    // feed (made with the Tauri CLI's signer and that key; the private key was thrown away).
    const PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXkgKGFubmFsbyBlMmUpClJXVGxVOEJ0eDQ1MC9XaEV1YjM3S0cvcTFlTjRWNkVDTlVhbHFqZXZPRHNrVjQxazVxSVlLNDQrCg==";
    const SIG: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIGFubmFsbyBlMmUga2V5ClJXVGxVOEJ0eDQ1MC9XNXRSSENVbUpGRDdYdEVSeGpwUit5OUJHWk1pZWhvS09vR0Z2SVRtUWtmT1o0WVZtNlQxbzQvdVdZT0NwYjlHSDdNL3VKWDY5UklhYkRrdGh4VEpnVT0KdHJ1c3RlZCBjb21tZW50OiB0aW1lc3RhbXA6MTc5MDkzMjMxNwlmaWxlOmFubmFsby11cGRhdGUuYmluCXZlcnNpb246MS45LjEKYVh2WGIwNXAvc01TTDJCcVNIdGlsazBBQzZnQ3loN0lXOU9qdTdBcyt4SHNKenJMQnFJUXpPYVUwY1lwbEJTLzNWRzUya0JWaFVYcjBjcGY1YU5ZQlE9PQo=";
    const PAYLOAD: &[u8] = b"arcalo update payload";

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("arcalo-mail-feed-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_feed_and_its_platforms() {
        let text = r#"{"version":"1.0.1","notes":"x","pub_date":"2026-10-20T10:00:00Z","platforms":{
            "windows-x86_64":{"url":"https://example.org/s.exe","signature":"s1"},
            "linux-x86_64":{"url":"https://example.org/a.AppImage","signature":"s2"},
            "future-arch":{"something":"else"}}}"#;
        let m = parse(text).unwrap();
        assert_eq!(m.platforms.len(), 2, "an entry this version cannot read is left out");
        assert_eq!(platform(&m, &targets("windows-x86_64", Some("nsis"))).unwrap().signature, "s1");
        assert_eq!(targets("linux-x86_64", None), ["linux-x86_64"]);
        assert!(platform(&m, &targets("darwin-aarch64", Some("app"))).is_none());
        assert!(matches!(parse(r#"{"version":"soon","platforms":{}}"#), Err(FeedError::Invalid(_))));
        assert!(matches!(parse("<html>"), Err(FeedError::Invalid(_))));
        assert!(parse("\u{feff}{\"version\":\"v1.0.1\"}").is_ok(), "a BOM and a v are fine");
    }

    #[test]
    fn signatures_are_checked_with_their_version() {
        verify(PAYLOAD, SIG, PUBKEY, "1.9.1").unwrap();
        verify(PAYLOAD, SIG, PUBKEY, "v1.9.1").unwrap();
        assert!(matches!(verify(PAYLOAD, SIG, PUBKEY, "1.9.2"), Err(FeedError::Signature(_))), "another version");
        assert!(matches!(verify(b"tampered", SIG, PUBKEY, "1.9.1"), Err(FeedError::Signature(_))));
        assert!(matches!(verify(PAYLOAD, "garbage", PUBKEY, "1.9.1"), Err(FeedError::Signature(_))));
    }

    /// A tiny HTTP server on loopback that serves `body` and honors `Range` (the first answer
    /// breaks off after `cut` bytes).
    async fn serve(body: Vec<u8>, cut: usize) -> String {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut first = true;
            loop {
                let Ok((mut s, _)) = listener.accept().await else { return };
                let mut buf = vec![0u8; 4096];
                let n = s.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let start = req
                    .lines()
                    .find_map(|l| l.to_ascii_lowercase().strip_prefix("range: bytes=").map(str::to_string))
                    .and_then(|r| r.trim_end_matches('-').trim().parse::<usize>().ok());
                let (head, part) = match start {
                    Some(st) => (
                        format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {st}-{}/{}\r\nConnection: close\r\n\r\n",
                            body.len() - st,
                            body.len() - 1,
                            body.len()
                        ),
                        body[st..].to_vec(),
                    ),
                    None => (
                        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()),
                        body.clone(),
                    ),
                };
                let _ = s.write_all(head.as_bytes()).await;
                let send = if first { &part[..cut.min(part.len())] } else { &part[..] };
                first = false;
                let _ = s.write_all(send).await;
                let _ = s.shutdown().await;
            }
        });
        format!("http://{addr}/update.bin")
    }

    #[tokio::test]
    async fn an_interrupted_download_continues_where_it_stopped() {
        let body: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let url = serve(body.clone(), 100_000).await;
        let client = crate::network::client_for(crate::network::Service::UpdateFeed).unwrap();
        let dir = tmp("download");
        let part = dir.join("1.0.1.part");
        let pause = AtomicBool::new(false);
        let first = download(&client, &url, &part, &pause, |_, _| {}).await;
        assert_eq!(first, Err(FeedError::Interrupted));
        let have = std::fs::metadata(&part).unwrap().len();
        assert!(have > 0 && have < body.len() as u64, "{have}");
        let mut last = (0, None);
        let second = download(&client, &url, &part, &pause, |d, t| last = (d, t)).await;
        assert_eq!(second, Ok(Step::Done));
        assert_eq!(std::fs::read(&part).unwrap(), body);
        assert_eq!(last, (body.len() as u64, Some(body.len() as u64)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_paused_download_keeps_its_part() {
        let body = vec![7u8; 50_000];
        let url = serve(body.clone(), usize::MAX).await;
        let client = crate::network::client_for(crate::network::Service::UpdateFeed).unwrap();
        let dir = tmp("pause");
        let part = dir.join("1.0.1.part");
        let pause = AtomicBool::new(true);
        assert_eq!(download(&client, &url, &part, &pause, |_, _| {}).await, Ok(Step::Paused));
        assert!(std::fs::metadata(&part).unwrap().len() > 0);
        pause.store(false, Ordering::SeqCst);
        assert_eq!(download(&client, &url, &part, &pause, |_, _| {}).await, Ok(Step::Done));
        assert_eq!(std::fs::read(&part).unwrap(), body);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn only_https_or_this_computer() {
        let client = crate::network::client_for(crate::network::Service::UpdateFeed).unwrap();
        let pause = AtomicBool::new(false);
        let part = std::env::temp_dir().join("arcalo-mail-never.part");
        let r = download(&client, "http://example.org/x.exe", &part, &pause, |_, _| {}).await;
        assert!(matches!(r, Err(FeedError::Refused(_))));
        assert!(matches!(fetch(&client, "ftp://example.org/latest.json").await, Err(FeedError::Refused(_))));
        assert!(!part.exists());
    }
}
