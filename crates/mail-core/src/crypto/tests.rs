//! Round trips with keys made per test: OpenPGP (PGP/MIME signed, encrypted and signed, inline,
//! Autocrypt) and S/MIME (a test authority and its users' certificates in PKCS#12 files), from
//! the message compose builds to what the reading pane gets; and the Web Key Directory's hash.

use std::collections::HashMap;

use super::*;
use crate::account::{AccountDraft, Provider, preset};
use crate::compose::{Draft, build};

/// The secrets of one person's device.
#[derive(Default)]
struct Vault {
    pgp: HashMap<String, String>,
    smime: HashMap<String, (Vec<u8>, String)>,
}

impl Keys for Vault {
    fn pgp_secret(&self, fp: &str) -> Option<String> {
        self.pgp.get(fp).cloned()
    }
    fn smime_secret(&self, fp: &str) -> Option<(Vec<u8>, String)> {
        self.smime.get(fp).cloned()
    }
}

/// One person: a store with an account for `email`, and the vault.
fn person(email: &str) -> (Store, Vault, crate::account::Account) {
    let s = Store::open_in_memory().unwrap();
    let id = s
        .add_account(&AccountDraft {
            provider: Provider::Imap,
            name: String::new(),
            email: email.into(),
            sender_name: String::new(),
            transport: preset(Provider::Imap, email).unwrap(),
        })
        .unwrap();
    let acc = s.account(id).unwrap();
    (s, Vault::default(), acc)
}

fn message(from: &crate::account::Account, to: &str, text: &str) -> Vec<u8> {
    let d = Draft {
        account_id: from.id,
        to: vec![to.into()],
        subject: "Vertrag".into(),
        text: text.into(),
        html: Some(format!("<p>{text}</p><img src=\"https://tracker.example/p.gif\">")),
        ..Draft::default()
    };
    build(from, &d, &[], "m1@example.org", 1_792_000_000, false).unwrap()
}

fn own_pgp_key(s: &Store, v: &mut Vault, email: &str) -> pgp::SignedPublicKey {
    let (secret, public) = pgp::generate("Test", email).unwrap();
    let info = save_pgp(s, &public, true, "generated", true).unwrap();
    v.pgp.insert(info.fingerprint, secret);
    public
}

#[test]
fn pgp_mime_signed_and_encrypted_round_trips() {
    let (maria, mut maria_keys, maria_acc) = person("maria@example.org");
    let (jonas, mut jonas_keys, jonas_acc) = person("jonas@example.org");
    let maria_pub = own_pgp_key(&maria, &mut maria_keys, "maria@example.org");
    let jonas_pub = own_pgp_key(&jonas, &mut jonas_keys, "jonas@example.org");
    let raw = message(&maria_acc, "jonas@example.org", "Hier die Zahlen: 42 €.");
    // Signed only: Jonas without Maria's key sees an unknown key, with it a good signature.
    let signed = protect(
        &maria,
        "maria@example.org",
        &raw,
        &["jonas@example.org".into()],
        &Protect { method: Method::Pgp, sign: true, encrypt: false },
        &maria_keys,
    )
    .unwrap();
    let text = String::from_utf8_lossy(&signed);
    assert!(text.contains("multipart/signed; micalg=pgp-sha256; protocol=\"application/pgp-signature\""), "{text}");
    assert!(text.contains("Subject: Vertrag") && text.contains("-----BEGIN PGP SIGNATURE-----"));
    let o = open(&jonas, &signed, Some("maria@example.org"), &jonas_keys).unwrap();
    assert_eq!((o.protection.signed, o.protection.signature.as_deref()), (true, Some("unknown_key")));
    save_pgp(&jonas, &maria_pub, false, "imported", false).unwrap();
    let o = open(&jonas, &signed, Some("maria@example.org"), &jonas_keys).unwrap();
    assert_eq!(o.protection.signature.as_deref(), Some("valid"));
    assert!(o.protection.signer.as_deref().unwrap().contains("maria@example.org"));
    let shown = crate::parse::parse(&o.entity.unwrap()).unwrap();
    assert!(shown.text.unwrap().contains("Hier die Zahlen: 42 €."));
    // From someone else's address: the signature is good but not the sender's.
    assert_eq!(
        open(&jonas, &signed, Some("lea@example.org"), &jonas_keys).unwrap().protection.signature.as_deref(),
        Some("unknown_key")
    );
    // Tampered on the way.
    let tampered = String::from_utf8_lossy(&signed)
        .replacen("SGllciBkaWUgWmFobGVu", "SGllciBkaWUgWmFobGVv", 1)
        .replacen("Hier die Zahlen", "Hier die Zahlem", 1);
    let o = open(&jonas, tampered.as_bytes(), Some("maria@example.org"), &jonas_keys).unwrap();
    assert_eq!(o.protection.signature.as_deref(), Some("invalid"));

    // Encrypted without Jonas's key: refused with his address.
    let how = Protect { method: Method::Pgp, sign: true, encrypt: true };
    let err = protect(&maria, "maria@example.org", &raw, &["jonas@example.org".into()], &how, &maria_keys)
        .unwrap_err()
        .to_string();
    assert!(err.contains("jonas@example.org"), "{err}");
    save_pgp(&maria, &jonas_pub, false, "wkd", false).unwrap();
    let abilities = abilities(&maria, "maria@example.org", &["jonas@example.org".into()]).unwrap();
    assert!(abilities.pgp_sign && abilities.pgp_encrypt && abilities.pgp_missing.is_empty());
    assert!(!abilities.smime_sign);
    let sealed = protect(&maria, "maria@example.org", &raw, &["jonas@example.org".into()], &how, &maria_keys).unwrap();
    let text = String::from_utf8_lossy(&sealed);
    assert!(
        text.contains("multipart/encrypted; protocol=\"application/pgp-encrypted\"") && !text.contains("Zahlen"),
        "{text}"
    );
    let o = open(&jonas, &sealed, Some("maria@example.org"), &jonas_keys).unwrap();
    assert!(o.protection.encrypted && o.protection.decrypted);
    assert_eq!(o.protection.signature.as_deref(), Some("valid"));
    assert!(crate::parse::parse(&o.entity.unwrap()).unwrap().html.unwrap().contains("Hier die Zahlen"));
    // Maria reads her copy in Sent; Lea cannot.
    assert!(open(&maria, &sealed, Some("maria@example.org"), &maria_keys).unwrap().protection.decrypted);
    let (lea, mut lea_keys, _) = person("lea@example.org");
    own_pgp_key(&lea, &mut lea_keys, "lea@example.org");
    let o = open(&lea, &sealed, Some("maria@example.org"), &lea_keys).unwrap();
    assert!(o.entity.is_none() && !o.protection.decrypted && o.protection.error.is_some());
    // A plain message is not touched.
    assert!(open(&jonas, &raw, Some("maria@example.org"), &jonas_keys).is_none());
    let _ = jonas_acc;
}

#[test]
fn inline_pgp_and_autocrypt() {
    let (maria, mut maria_keys, maria_acc) = person("maria@example.org");
    let (jonas, mut jonas_keys, _) = person("jonas@example.org");
    let maria_pub = own_pgp_key(&maria, &mut maria_keys, "maria@example.org");
    let jonas_pub = own_pgp_key(&jonas, &mut jonas_keys, "jonas@example.org");
    // Autocrypt: Maria's header carries her key; Jonas keeps it for her address only.
    let header = autocrypt_for(&maria, "maria@example.org").unwrap().unwrap();
    assert!(header.starts_with("addr=maria@example.org; prefer-encrypt=mutual; keydata="));
    let raw = add_header(&message(&maria_acc, "jonas@example.org", "Hallo"), "Autocrypt", &header);
    ingest_autocrypt(&jonas, &raw, "maria@example.org").unwrap();
    assert_eq!(pgp_keys_for(&jonas, "maria@example.org").unwrap().len(), 1);
    let (lea, _, _) = person("lea@example.org");
    ingest_autocrypt(&lea, &raw, "someone@else.org").unwrap();
    assert!(pgp_keys_for(&lea, "maria@example.org").unwrap().is_empty(), "only for the sender's own address");
    assert!(autocrypt_for(&lea, "lea@example.org").unwrap().is_none(), "no own key, no header");
    // Inline: an encrypted block in a text mail.
    let secret = pgp::read_secret(maria_keys.pgp.values().next().unwrap()).unwrap();
    let secret = pgp::Secret::plain(secret);
    let armored = pgp::encrypt("Geheim: 1234".as_bytes(), &[jonas_pub], Some(&secret)).unwrap();
    let body = format!("Hallo Jonas,\n\n{armored}\n");
    let (text, p) = open_inline(&jonas, &body, Some("maria@example.org"), &jonas_keys).unwrap();
    assert_eq!((text.as_str(), p.decrypted, p.signature.as_deref()), ("Geheim: 1234", true, Some("valid")));
    // A cleartext signature.
    let signed = ::pgp::composed::CleartextSignedMessage::sign(
        rand::thread_rng(),
        "Bitte bestätigen.\n",
        &secret.key.primary_key,
        &::pgp::types::Password::empty(),
    )
    .unwrap()
    .to_armored_string(::pgp::composed::ArmorOptions::default())
    .unwrap();
    let (text, p) = open_inline(&jonas, &signed, Some("maria@example.org"), &jonas_keys).unwrap();
    assert_eq!((text.trim(), p.signature.as_deref()), ("Bitte bestätigen.", Some("valid")));
    assert!(open_inline(&jonas, "nur Text", None, &jonas_keys).is_none());
    let _ = maria_pub;
}

/// A test authority and a certificate for `email` signed by it, as a PKCS#12 file.
fn smime_identity(ca: &(rcgen::Certificate, rcgen::KeyPair, rcgen::CertificateParams), email: &str) -> Vec<u8> {
    use rcgen::{CertificateParams, DnType, ExtendedKeyUsagePurpose, KeyPair, PKCS_RSA_SHA256, RsaKeySize, SanType};
    let key = KeyPair::generate_rsa_for(&PKCS_RSA_SHA256, RsaKeySize::_2048).unwrap();
    let mut params = CertificateParams::new(vec![]).unwrap();
    params.distinguished_name.push(DnType::CommonName, email.split('@').next().unwrap());
    params.subject_alt_names = vec![SanType::Rfc822Name(email.try_into().unwrap())];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::EmailProtection];
    let issuer = rcgen::Issuer::from_params(&ca.2, &ca.1);
    let cert = params.signed_by(&key, &issuer).unwrap();
    let chain = p12_keystore::PrivateKeyChain::new(
        key.serialize_der(),
        [1u8; 20],
        vec![
            p12_keystore::Certificate::from_der(cert.der()).unwrap(),
            p12_keystore::Certificate::from_der(ca.0.der()).unwrap(),
        ],
    );
    let mut ks = p12_keystore::KeyStore::new();
    ks.add_entry("me", p12_keystore::KeyStoreEntry::PrivateKeyChain(chain));
    ks.writer("geheim").write().unwrap()
}

fn test_ca() -> (rcgen::Certificate, rcgen::KeyPair, rcgen::CertificateParams) {
    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair, PKCS_RSA_SHA256, RsaKeySize};
    let key = KeyPair::generate_rsa_for(&PKCS_RSA_SHA256, RsaKeySize::_2048).unwrap();
    let mut params = CertificateParams::new(vec![]).unwrap();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.distinguished_name.push(DnType::CommonName, "Arcalo Test CA");
    let cert = params.self_signed(&key).unwrap();
    (cert, key, params)
}

fn own_smime(s: &Store, v: &mut Vault, p12: &[u8]) -> smime::CertInfo {
    let id = smime::read_p12(p12, "geheim").unwrap();
    let info = save_cert(s, &id.cert, true, true).unwrap();
    v.smime.insert(info.fingerprint.clone(), (p12.to_vec(), "geheim".into()));
    info
}

#[test]
fn smime_signed_and_encrypted_round_trips() {
    let ca = test_ca();
    let (maria, mut maria_keys, maria_acc) = person("maria@example.org");
    let (jonas, mut jonas_keys, _) = person("jonas@example.org");
    let maria_p12 = smime_identity(&ca, "maria@example.org");
    assert!(smime::read_p12(&maria_p12, "falsch").is_err());
    let info = own_smime(&maria, &mut maria_keys, &maria_p12);
    assert_eq!(
        (info.emails.as_slice(), info.issuer.as_str()),
        (["maria@example.org".to_string()].as_slice(), "Arcalo Test CA")
    );
    let jonas_info = own_smime(&jonas, &mut jonas_keys, &smime_identity(&ca, "jonas@example.org"));
    let raw = message(&maria_acc, "jonas@example.org", "Angebot anbei.");
    let signed = protect(
        &maria,
        "maria@example.org",
        &raw,
        &[],
        &Protect { method: Method::Smime, sign: true, encrypt: false },
        &maria_keys,
    )
    .unwrap();
    assert!(String::from_utf8_lossy(&signed).contains("application/pkcs7-signature"));
    // Good but not trusted: the test authority is no root of this device yet.
    let o = open(&jonas, &signed, Some("maria@example.org"), &jonas_keys).unwrap();
    assert_eq!((o.protection.method.as_str(), o.protection.signature.as_deref()), ("smime", Some("untrusted")));
    assert!(crate::parse::parse(&o.entity.unwrap()).unwrap().text.unwrap().contains("Angebot anbei."));
    assert!(certs_for(&jonas, "maria@example.org").unwrap().is_empty(), "an untrusted certificate is not kept");
    smime::trust_for_tests(ca.0.der().to_vec());
    let o = open(&jonas, &signed, Some("maria@example.org"), &jonas_keys).unwrap();
    assert_eq!(o.protection.signature.as_deref(), Some("valid"));
    assert_eq!(certs_for(&jonas, "maria@example.org").unwrap().len(), 1, "the sender's trusted certificate is kept");
    assert_eq!(
        open(&jonas, &signed, Some("lea@example.org"), &jonas_keys).unwrap().protection.signature.as_deref(),
        Some("mismatch")
    );
    let tampered = String::from_utf8_lossy(&signed).replacen("Angebot anbei", "Angebot anbeo", 1);
    assert_eq!(
        open(&jonas, tampered.as_bytes(), Some("maria@example.org"), &jonas_keys)
            .unwrap()
            .protection
            .signature
            .as_deref(),
        Some("invalid")
    );

    // Encrypted and signed: Maria has Jonas's certificate from his signed mail.
    save_cert(&maria, &certs_for(&jonas, "jonas@example.org").unwrap()[0], false, true).unwrap();
    let how = Protect { method: Method::Smime, sign: true, encrypt: true };
    let sealed = protect(&maria, "maria@example.org", &raw, &["jonas@example.org".into()], &how, &maria_keys).unwrap();
    let text = String::from_utf8_lossy(&sealed);
    assert!(text.contains("smime-type=enveloped-data") && !text.contains("Angebot"), "{text}");
    let o = open(&jonas, &sealed, Some("maria@example.org"), &jonas_keys).unwrap();
    assert!(o.protection.encrypted && o.protection.decrypted);
    assert_eq!(o.protection.signature.as_deref(), Some("valid"));
    assert!(crate::parse::parse(&o.entity.unwrap()).unwrap().text.unwrap().contains("Angebot anbei."));
    assert!(
        open(&maria, &sealed, Some("maria@example.org"), &maria_keys).unwrap().protection.decrypted,
        "the sender's copy"
    );
    let (lea, lea_keys, _) = person("lea@example.org");
    let o = open(&lea, &sealed, Some("maria@example.org"), &lea_keys).unwrap();
    assert!(o.entity.is_none() && o.protection.error.is_some());
    let err = protect(&maria, "maria@example.org", &raw, &["lea@example.org".into()], &how, &maria_keys)
        .unwrap_err()
        .to_string();
    assert!(err.contains("lea@example.org"));
    let listed = list(&maria).unwrap();
    assert!(listed.iter().any(|k| k.kind == "smime" && k.own && k.email == "maria@example.org"));
    assert!(listed.iter().any(|k| k.kind == "smime" && !k.own && k.fingerprint == jonas_info.fingerprint));
}

#[test]
fn wkd_addresses_and_lookup() {
    // The example of the WKD draft.
    let urls = wkd::urls("Joe.Doe@Example.ORG").unwrap();
    assert_eq!(
        urls[0],
        "https://openpgpkey.example.org/.well-known/openpgpkey/example.org/hu/iy9q119eutrkn8s1mk4r39qejnbu3n5q?l=Joe.Doe"
    );
    assert_eq!(urls[1], "https://example.org/.well-known/openpgpkey/hu/iy9q119eutrkn8s1mk4r39qejnbu3n5q?l=Joe.Doe");
    assert!(wkd::urls("kein").is_none());
}

#[tokio::test]
async fn wkd_fetches_the_first_address_that_has_the_key() {
    use crate::network::tests::testhttp::{Reply, serve};
    use std::sync::Arc;
    let (_, public) = pgp::generate("Jonas", "jonas@example.org").unwrap();
    let bytes = pgp::binary_public(&public).unwrap();
    let port = serve(Arc::new(move |_m: &str, path: &str, _h: &[(String, String)], _b: &[u8]| {
        if path.starts_with("/direct/") {
            Reply {
                status: 200,
                content_type: "application/octet-stream".into(),
                body: bytes.clone(),
                location: None,
                headers: vec![],
            }
        } else {
            Reply::status(404, "")
        }
    }))
    .await;
    let got = wkd::fetch(&[format!("http://127.0.0.1:{port}/advanced/x"), format!("http://127.0.0.1:{port}/direct/x")])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pgp::fingerprint_of(&pgp::read_public(&got).unwrap()), pgp::fingerprint_of(&public));
    assert!(wkd::fetch(&[format!("http://127.0.0.1:{port}/none")]).await.unwrap().is_none());
}

#[test]
fn parts_of_a_message_with_mixed_line_ends() {
    // OpenSSL writes the header with LF and the signed part with CRLF.
    let raw = b"MIME-Version: 1.0\nContent-Type: multipart/signed; protocol=\"application/x-pkcs7-signature\"; micalg=\"sha-256\"; boundary=\"----B\"\n\nThis is an S/MIME signed message\n\n------B\nContent-Type: text/plain\r\n\r\nHallo\r\n\n------B\nContent-Type: application/x-pkcs7-signature\n\nAAAA\n\n------B--\n";
    let (mime, params) = content_type(raw);
    assert_eq!(mime, "multipart/signed");
    let list = parts(raw, param(&params, "boundary").unwrap());
    assert_eq!(list.len(), 2);
    assert_eq!(list[0], b"Content-Type: text/plain\r\n\r\nHallo\r\n");
}

#[test]
fn the_reading_pane_opens_encrypted_mail_without_remote_content() {
    use crate::imap::MailboxEntry;
    use crate::settings::RemoteContent;
    let (maria, mut maria_keys, maria_acc) = person("maria@example.org");
    let (jonas, mut jonas_keys, _) = person("jonas@example.org");
    let maria_pub = own_pgp_key(&maria, &mut maria_keys, "maria@example.org");
    let jonas_pub = own_pgp_key(&jonas, &mut jonas_keys, "jonas@example.org");
    save_pgp(&maria, &jonas_pub, false, "imported", false).unwrap();
    save_pgp(&jonas, &maria_pub, false, "imported", false).unwrap();
    let d = Draft {
        account_id: maria_acc.id,
        to: vec!["jonas@example.org".into()],
        subject: "Vertrag".into(),
        text: "Bitte lesen. Einwahl: https://meet.jit.si/Vertrag".into(),
        html: Some("<p>Bitte lesen.</p><img src=\"https://cdn.example.org/logo.png\" width=\"200\">".into()),
        ..Draft::default()
    };
    let mut raw = build(
        &maria_acc,
        &d,
        &[crate::compose::FileAttachment {
            name: "vertrag.pdf".into(),
            mime_type: "application/pdf".into(),
            data: b"%PDF-1.4".to_vec(),
        }],
        "v1@example.org",
        1_792_000_000,
        false,
    )
    .unwrap();
    raw = protect(
        &maria,
        "maria@example.org",
        &raw,
        &["jonas@example.org".into()],
        &Protect { method: Method::Pgp, sign: true, encrypt: true },
        &maria_keys,
    )
    .unwrap();
    let inbox = jonas
        .upsert_mailbox(
            jonas.accounts().unwrap()[0].id,
            &MailboxEntry { name: "INBOX".into(), delimiter: Some('/'), attributes: vec![] },
        )
        .unwrap();
    let p = crate::parse::parse(&raw).unwrap();
    let id = jonas.insert_message(inbox, &crate::store::tests::header(1, &raw), &p).unwrap();
    jonas.set_body_raw(id, &p, Some(&raw)).unwrap();
    jonas.allow_sender("maria@example.org").unwrap();
    let v = crate::actions::message_view_with(&jonas, id, RemoteContent::Always, &jonas_keys).unwrap();
    let prot = v.protection.clone().unwrap();
    assert!(prot.encrypted && prot.decrypted && prot.signature.as_deref() == Some("valid"));
    assert!(v.body.html.contains("Bitte lesen."));
    assert!(!v.body.html.contains("cdn.example.org"), "never remote content in decrypted mail");
    assert!(v.files.is_empty(), "the wrapper parts are not files");
    assert_eq!(
        v.protected_files.iter().map(|f| f.filename.as_deref().unwrap_or_default()).collect::<Vec<_>>(),
        ["vertrag.pdf"]
    );
    assert_eq!(v.meeting.as_ref().map(|m| m.kind.as_str()), Some("Jitsi"));
    // Without the key: the state says so, nothing of the content shows.
    let v = crate::actions::message_view_with(&jonas, id, RemoteContent::Ask, &NoKeys).unwrap();
    let prot = v.protection.unwrap();
    assert!(prot.encrypted && !prot.decrypted && prot.error.is_some());
    assert!(!v.body.html.contains("Bitte lesen."));
}

// ---------------------------------------------------------------------------- key trust

/// A key made here with these user ids (the first is the primary one), its secret part.
fn key_with_users(users: &[&str], passphrase: Option<&str>) -> pgp::SignedSecretKey {
    use ::pgp::composed::{EncryptionCaps, KeyType, SecretKeyParamsBuilder, SubkeyParamsBuilder};
    use ::pgp::crypto::ecc_curve::ECCCurve;
    let mut enc = SubkeyParamsBuilder::default();
    enc.key_type(KeyType::ECDH(ECCCurve::Curve25519Legacy))
        .can_sign(false)
        .can_encrypt(EncryptionCaps::All)
        .passphrase(passphrase.map(str::to_string));
    let mut params = SecretKeyParamsBuilder::default();
    params
        .key_type(KeyType::Ed25519Legacy)
        .can_certify(true)
        .can_sign(true)
        .can_encrypt(EncryptionCaps::None)
        .passphrase(passphrase.map(str::to_string))
        .primary_user_id(users[0].into())
        .user_ids(users[1..].iter().map(|u| u.to_string()).collect())
        .subkeys(vec![enc.build().unwrap()]);
    params.build().unwrap().generate(rand::thread_rng()).unwrap()
}

/// A mail from `from` dated `date` with an Autocrypt header for `addr` carrying `key`.
fn autocrypt_mail(from: &str, addr: &str, key: &pgp::SignedPublicKey, date: &str) -> Vec<u8> {
    let header = pgp::autocrypt_header(addr, key, true).unwrap();
    format!(
        "From: <{from}>\r\nTo: jonas@example.org\r\nDate: {date}\r\nAutocrypt: {header}\r\nSubject: Hi\r\n\r\nHallo\r\n"
    )
    .into_bytes()
}

fn fingerprints(keys: &[pgp::SignedPublicKey]) -> Vec<String> {
    keys.iter().map(pgp::fingerprint_of).collect()
}

#[test]
fn autocrypt_files_a_key_under_the_sending_address_only() {
    // Mallory's key names Maria first and his own address second; his mail with a correct
    // Autocrypt header must not make it a key of Maria.
    let (jonas, jonas_keys, _) = person("jonas@example.org");
    let mallory = key_with_users(&["Maria <maria@example.org>", "<mallory@evil.example>"], None);
    let mallory_pub = mallory.to_public_key();
    let raw =
        autocrypt_mail("mallory@evil.example", "mallory@evil.example", &mallory_pub, "Mon, 1 Jun 2026 10:00:00 +0000");
    ingest_autocrypt(&jonas, &raw, "mallory@evil.example").unwrap();
    assert!(pgp_keys_for(&jonas, "maria@example.org").unwrap().is_empty(), "not a key of Maria");
    let listed = list(&jonas).unwrap();
    assert_eq!(listed.iter().find(|k| k.source == "autocrypt").map(|k| k.email.as_str()), Some("mallory@evil.example"));
    assert_eq!(
        fingerprints(&pgp_keys_for(&jonas, "mallory@evil.example").unwrap()),
        [pgp::fingerprint_of(&mallory_pub)]
    );
    // A spoofed „From: Maria“ signed with Mallory's key is no valid signature of Maria.
    let signed = ::pgp::composed::CleartextSignedMessage::sign(
        rand::thread_rng(),
        "Bitte 5000 EUR überweisen.\n",
        &mallory.primary_key,
        &::pgp::types::Password::empty(),
    )
    .unwrap()
    .to_armored_string(::pgp::composed::ArmorOptions::default())
    .unwrap();
    let (_, p) = open_inline(&jonas, &signed, Some("maria@example.org"), &jonas_keys).unwrap();
    assert_ne!(p.signature.as_deref(), Some("valid"));
    // Signed as Mallory, it is Mallory's.
    let (_, p) = open_inline(&jonas, &signed, Some("mallory@evil.example"), &jonas_keys).unwrap();
    assert_eq!(p.signature.as_deref(), Some("valid"));
}

#[test]
fn autocrypt_key_for_a_second_user_id_serves_that_address() {
    let (maria, _, _) = person("maria@example.org");
    let lea = key_with_users(&["Lea <lea@private.example>", "Lea <lea@corp.example>"], None).to_public_key();
    let raw = autocrypt_mail("lea@corp.example", "lea@corp.example", &lea, "Mon, 1 Jun 2026 10:00:00 +0000");
    ingest_autocrypt(&maria, &raw, "lea@corp.example").unwrap();
    let a = abilities(&maria, "maria@example.org", &["lea@corp.example".into()]).unwrap();
    assert!(a.pgp_missing.is_empty(), "{:?}", a.pgp_missing);
    assert!(pgp_keys_for(&maria, "lea@private.example").unwrap().is_empty(), "only the address it came from");
}

#[test]
fn autocrypt_follows_the_message_date_and_never_outranks_an_imported_key() {
    let (jonas, _, _) = person("jonas@example.org");
    let old = key_with_users(&["<maria@example.org>"], None).to_public_key();
    let new = key_with_users(&["<maria@example.org>"], None).to_public_key();
    // The newer mail brings the new key; opening an older mail afterwards changes nothing.
    let june = autocrypt_mail("maria@example.org", "maria@example.org", &new, "Mon, 1 Jun 2026 10:00:00 +0000");
    let march = autocrypt_mail("maria@example.org", "maria@example.org", &old, "Mon, 2 Mar 2026 10:00:00 +0000");
    ingest_autocrypt(&jonas, &june, "maria@example.org").unwrap();
    ingest_autocrypt(&jonas, &march, "maria@example.org").unwrap();
    assert_eq!(fingerprints(&pgp_keys_for(&jonas, "maria@example.org").unwrap()), [pgp::fingerprint_of(&new)]);
    // A header without a date counts for nothing.
    let undated = format!(
        "From: <maria@example.org>\r\nAutocrypt: {}\r\nSubject: x\r\n\r\nx\r\n",
        pgp::autocrypt_header("maria@example.org", &old, true).unwrap()
    );
    ingest_autocrypt(&jonas, undated.as_bytes(), "maria@example.org").unwrap();
    assert_eq!(fingerprints(&pgp_keys_for(&jonas, "maria@example.org").unwrap()), [pgp::fingerprint_of(&new)]);
    // The key Jonas imported comes first, whatever Autocrypt brings later.
    let imported = key_with_users(&["Maria <maria@example.org>"], None).to_public_key();
    save_pgp(&jonas, &imported, false, "imported", false).unwrap();
    let later = key_with_users(&["<maria@example.org>"], None).to_public_key();
    let july = autocrypt_mail("maria@example.org", "maria@example.org", &later, "Wed, 1 Jul 2026 10:00:00 +0000");
    ingest_autocrypt(&jonas, &july, "maria@example.org").unwrap();
    let keys = fingerprints(&pgp_keys_for(&jonas, "maria@example.org").unwrap());
    assert_eq!(keys, [pgp::fingerprint_of(&imported), pgp::fingerprint_of(&later)]);
    // Autocrypt bringing the imported key itself leaves it imported.
    let again = autocrypt_mail("maria@example.org", "maria@example.org", &imported, "Sat, 1 Aug 2026 10:00:00 +0000");
    ingest_autocrypt(&jonas, &again, "maria@example.org").unwrap();
    let listed = list(&jonas).unwrap();
    let row = listed.iter().find(|k| k.fingerprint == pgp::fingerprint_of(&imported)).unwrap();
    assert_eq!(row.source, "imported");
}

#[test]
fn autocrypt_prefer_encrypt_starts_the_compose_sheet_encrypted() {
    let (maria, mut maria_keys, _) = person("maria@example.org");
    own_pgp_key(&maria, &mut maria_keys, "maria@example.org");
    let jonas = key_with_users(&["<jonas@example.org>"], None).to_public_key();
    let raw = autocrypt_mail("jonas@example.org", "jonas@example.org", &jonas, "Mon, 1 Jun 2026 10:00:00 +0000");
    ingest_autocrypt(&maria, &raw, "jonas@example.org").unwrap();
    let a = abilities(&maria, "maria@example.org", &["jonas@example.org".into()]).unwrap();
    assert!(a.pgp_encrypt && a.pgp_prefer_encrypt);
    let lea = key_with_users(&["<lea@example.org>"], None).to_public_key();
    save_pgp(&maria, &lea, false, "imported", false).unwrap();
    let a = abilities(&maria, "maria@example.org", &["jonas@example.org".into(), "lea@example.org".into()]).unwrap();
    assert!(a.pgp_encrypt && !a.pgp_prefer_encrypt, "Lea did not ask for it");
}

/// A message from Maria with a text part and an HTML part, stored for Jonas; returns its id.
fn stored(jonas: &Store, raw: &[u8]) -> i64 {
    use crate::imap::MailboxEntry;
    let inbox = jonas
        .upsert_mailbox(
            jonas.accounts().unwrap()[0].id,
            &MailboxEntry { name: "INBOX".into(), delimiter: Some('/'), attributes: vec![] },
        )
        .unwrap();
    let p = crate::parse::parse(raw).unwrap();
    let id = jonas.insert_message(inbox, &crate::store::tests::header(1, raw), &p).unwrap();
    jonas.set_body_raw(id, &p, Some(raw)).unwrap();
    id
}

fn alternative(text: &str, html: &str) -> Vec<u8> {
    format!(
        "From: Maria <maria@example.org>\r\nTo: jonas@example.org\r\nSubject: Bitte\r\nDate: Mon, 1 Jun 2026 10:00:00 +0000\r\nMIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{text}\r\n--b\r\nContent-Type: text/html; charset=utf-8\r\n\r\n{html}\r\n--b--\r\n"
    )
    .into_bytes()
}

#[test]
fn an_inline_signature_badges_only_the_signed_text() {
    use crate::settings::RemoteContent;
    let (maria, mut maria_keys, _) = person("maria@example.org");
    let (jonas, jonas_keys, _) = person("jonas@example.org");
    let maria_pub = own_pgp_key(&maria, &mut maria_keys, "maria@example.org");
    save_pgp(&jonas, &maria_pub, false, "imported", false).unwrap();
    let secret = pgp::read_secret(maria_keys.pgp.values().next().unwrap()).unwrap();
    let signed = ::pgp::composed::CleartextSignedMessage::sign(
        rand::thread_rng(),
        "ok\n",
        &secret.primary_key,
        &::pgp::types::Password::empty(),
    )
    .unwrap()
    .to_armored_string(::pgp::composed::ArmorOptions::default())
    .unwrap();
    // The signed text part next to an unsigned HTML part: the badge goes with the signed text,
    // and the unsigned HTML is not what is shown.
    let raw = alternative(&signed, "<p>Please wire 5000 EUR to DE00 1234 today. Maria</p>");
    let v = crate::actions::message_view_with(&jonas, stored(&jonas, &raw), RemoteContent::Ask, &jonas_keys).unwrap();
    assert_eq!(v.protection.as_ref().unwrap().signature.as_deref(), Some("valid"));
    assert!(!v.body.html.contains("5000 EUR"), "{}", v.body.html);
    assert!(v.body.html.contains("ok"));
    // Unsigned text around the signed block: partly signed, never „valid“.
    let raw = alternative(&format!("Please wire 5000 EUR today.\n\n{signed}"), "<p>x</p>");
    let v = crate::actions::message_view_with(&jonas, stored(&jonas, &raw), RemoteContent::Ask, &jonas_keys).unwrap();
    let p = v.protection.unwrap();
    assert_eq!((p.signature.as_deref(), p.partial), (Some("partial"), true));
}

/// What a passphrase-protected key's owner gave: the passphrase, or nothing yet.
struct WithPassphrase<'a>(&'a Vault, Option<&'a str>);

impl Keys for WithPassphrase<'_> {
    fn pgp_secret(&self, fp: &str) -> Option<String> {
        self.0.pgp_secret(fp)
    }
    fn smime_secret(&self, fp: &str) -> Option<(Vec<u8>, String)> {
        self.0.smime_secret(fp)
    }
    fn pgp_passphrase(&self, _: &str) -> Option<String> {
        self.1.map(str::to_string)
    }
}

#[test]
fn a_protected_secret_key_works_with_its_passphrase() {
    let (maria, mut maria_keys, maria_acc) = person("maria@example.org");
    let (jonas, mut jonas_keys, _) = person("jonas@example.org");
    let key = key_with_users(&["Maria <maria@example.org>"], Some("geheim"));
    assert!(pgp::is_protected(&key));
    assert!(pgp::unlocks(&key, "geheim") && !pgp::unlocks(&key, "falsch"));
    let public = key.to_public_key();
    let info = save_pgp(&maria, &public, true, "imported", true).unwrap();
    maria_keys.pgp.insert(info.fingerprint.clone(), key.to_armored_string(Default::default()).unwrap());
    let jonas_pub = own_pgp_key(&jonas, &mut jonas_keys, "jonas@example.org");
    save_pgp(&maria, &jonas_pub, false, "imported", false).unwrap();
    save_pgp(&jonas, &public, false, "imported", false).unwrap();
    let raw = message(&maria_acc, "jonas@example.org", "Hallo");
    let sign = Protect { method: Method::Pgp, sign: true, encrypt: false };
    // Without the passphrase: the key is named as locked.
    let locked = WithPassphrase(&maria_keys, None);
    let err = protect(&maria, "maria@example.org", &raw, &[], &sign, &locked).unwrap_err().to_string();
    assert!(err.contains(&info.fingerprint), "{err}");
    assert_eq!(locked_pgp(&maria, "maria@example.org", &locked).unwrap(), Some(info.fingerprint.clone()));
    let wrong = WithPassphrase(&maria_keys, Some("falsch"));
    assert!(protect(&maria, "maria@example.org", &raw, &[], &sign, &wrong).is_err());
    // With it: signed, and a signature Jonas can check.
    let unlocked = WithPassphrase(&maria_keys, Some("geheim"));
    assert_eq!(locked_pgp(&maria, "maria@example.org", &unlocked).unwrap(), None);
    let signed = protect(&maria, "maria@example.org", &raw, &[], &sign, &unlocked).unwrap();
    let o = open(&jonas, &signed, Some("maria@example.org"), &jonas_keys).unwrap();
    assert_eq!(o.protection.signature.as_deref(), Some("valid"));
    // Encrypted to Maria: locked, it says which key needs the passphrase; unlocked, it opens.
    let to_maria = protect(
        &jonas,
        "jonas@example.org",
        &message(&maria_acc, "maria@example.org", "Geheim"),
        &["maria@example.org".into()],
        &Protect { method: Method::Pgp, sign: false, encrypt: true },
        &jonas_keys,
    )
    .unwrap();
    let o = open(&maria, &to_maria, Some("jonas@example.org"), &locked).unwrap();
    assert!(!o.protection.decrypted);
    assert_eq!(o.protection.locked.as_deref(), Some(info.fingerprint.as_str()));
    let o = open(&maria, &to_maria, Some("jonas@example.org"), &unlocked).unwrap();
    assert!(o.protection.decrypted && o.protection.locked.is_none());
    // Encrypting only needs the public part: no passphrase asked.
    let enc = Protect { method: Method::Pgp, sign: false, encrypt: true };
    assert!(protect(&maria, "maria@example.org", &raw, &["jonas@example.org".into()], &enc, &locked).is_ok());
}

#[test]
fn bcc_recipients_get_their_own_encrypted_copy() {
    use base64::Engine;
    let b64 = |s: &str| base64::engine::general_purpose::STANDARD.decode(s).unwrap();
    let (maria, mut maria_keys, maria_acc) = person("maria@example.org");
    own_pgp_key(&maria, &mut maria_keys, "maria@example.org");
    let (jonas, mut jonas_keys, _) = person("jonas@example.org");
    let (lea, mut lea_keys, _) = person("lea@example.org");
    let jonas_pub = own_pgp_key(&jonas, &mut jonas_keys, "jonas@example.org");
    let lea_pub = own_pgp_key(&lea, &mut lea_keys, "lea@example.org");
    save_pgp(&maria, &jonas_pub, false, "imported", false).unwrap();
    save_pgp(&maria, &lea_pub, false, "imported", false).unwrap();
    let d = Draft {
        account_id: maria_acc.id,
        to: vec!["jonas@example.org".into()],
        bcc: vec!["lea@example.org".into()],
        subject: "Geheim".into(),
        text: "x".into(),
        protect: Some(Protect { method: Method::Pgp, sign: false, encrypt: true }),
        ..Draft::default()
    };
    let (op, _) = crate::actions::prepare_send_with(&maria, &d, &[], 1_792_000_000, &maria_keys).unwrap();
    let crate::outbox::Op::Send { raw_b64, recipients, copies, sent_b64, .. } = op else { panic!() };
    assert_eq!(recipients, ["jonas@example.org"], "the Bcc recipient is not in this envelope");
    let to_copy = b64(&raw_b64);
    assert!(open(&jonas, &to_copy, Some("maria@example.org"), &jonas_keys).unwrap().protection.decrypted);
    assert!(
        !open(&lea, &to_copy, Some("maria@example.org"), &lea_keys).unwrap().protection.decrypted,
        "the To copy is not encrypted for the Bcc recipient"
    );
    assert_eq!(copies.len(), 1);
    assert_eq!(copies[0].recipients, ["lea@example.org"]);
    let bcc_copy = b64(&copies[0].raw_b64);
    assert!(open(&lea, &bcc_copy, Some("maria@example.org"), &lea_keys).unwrap().protection.decrypted);
    assert!(!open(&jonas, &bcc_copy, Some("maria@example.org"), &jonas_keys).unwrap().protection.decrypted);
    assert!(!String::from_utf8_lossy(&bcc_copy).contains("lea@example.org"), "no Bcc header");
    assert!(
        open(&maria, &b64(&sent_b64.unwrap()), Some("maria@example.org"), &maria_keys).unwrap().protection.decrypted
    );
    // Not encrypted: one message, the Bcc recipient in its envelope.
    let plain = Draft { protect: None, ..d };
    let (op, _) = crate::actions::prepare_send_with(&maria, &plain, &[], 1_792_000_000, &maria_keys).unwrap();
    let crate::outbox::Op::Send { recipients, copies, .. } = op else { panic!() };
    assert_eq!((recipients.len(), copies.len()), (2, 0));
}

/// A self-signed certificate for `email` (until the year `until`) and its key, as PKCS#12.
fn self_signed(email: &str, until: Option<i32>) -> (Vec<u8>, Vec<u8>) {
    use rcgen::{CertificateParams, DnType, ExtendedKeyUsagePurpose, KeyPair, PKCS_RSA_SHA256, RsaKeySize, SanType};
    let key = KeyPair::generate_rsa_for(&PKCS_RSA_SHA256, RsaKeySize::_2048).unwrap();
    let mut params = CertificateParams::new(vec![]).unwrap();
    params.distinguished_name.push(DnType::CommonName, email);
    params.subject_alt_names = vec![SanType::Rfc822Name(email.try_into().unwrap())];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::EmailProtection];
    if let Some(y) = until {
        params.not_after = rcgen::date_time_ymd(y, 1, 1);
    }
    let cert = params.self_signed(&key).unwrap();
    let der = cert.der().to_vec();
    let chain = p12_keystore::PrivateKeyChain::new(
        key.serialize_der(),
        [1u8; 20],
        vec![p12_keystore::Certificate::from_der(&der).unwrap()],
    );
    let mut ks = p12_keystore::KeyStore::new();
    ks.add_entry("me", p12_keystore::KeyStoreEntry::PrivateKeyChain(chain));
    (der, ks.writer("geheim").write().unwrap())
}

#[test]
fn an_untrusted_certificate_from_a_mail_is_used_only_once_the_user_trusts_it() {
    let (maria, maria_keys, _) = person("maria@example.org");
    // Maria knows the boss's real certificate (imported, until 2030).
    let (real, _) = self_signed("boss@corp.example", Some(2030));
    save_cert(&maria, &real, false, true).unwrap();
    // Someone forges „From: boss“ and signs with a self-signed certificate of their own.
    let (evil, mut evil_keys, evil_acc) = person("boss@corp.example");
    let (evil_cert, evil_p12) = self_signed("boss@corp.example", None);
    own_smime(&evil, &mut evil_keys, &evil_p12);
    let raw = message(&evil_acc, "maria@example.org", "Bitte überweisen.");
    let signed = protect(
        &evil,
        "boss@corp.example",
        &raw,
        &[],
        &Protect { method: Method::Smime, sign: true, encrypt: false },
        &evil_keys,
    )
    .unwrap();
    let o = open(&maria, &signed, Some("boss@corp.example"), &maria_keys).unwrap();
    assert_eq!(o.protection.signature.as_deref(), Some("untrusted"));
    assert_eq!(
        certs_for(&maria, "boss@corp.example").unwrap(),
        std::slice::from_ref(&real),
        "the forged one is not used"
    );
    // The user checked it and trusts it: from now on it is a certificate of the address.
    let info = trust_signer(&maria, &signed, "boss@corp.example", &maria_keys).unwrap();
    assert_eq!(info.emails, ["boss@corp.example"]);
    assert!(certs_for(&maria, "boss@corp.example").unwrap().contains(&evil_cert));
    // Never for a message signed by someone else than its sender.
    assert!(trust_signer(&maria, &signed, "other@corp.example", &maria_keys).is_err());
}

/// A subkey binding (or revocation) of `sub` made with the primary secret key.
fn bind(
    secret: &pgp::SignedSecretKey,
    sub: &::pgp::packet::PublicSubkey,
    typ: ::pgp::packet::SignatureType,
    flags: Option<::pgp::packet::KeyFlags>,
    expires_after: Option<u32>,
) -> ::pgp::packet::Signature {
    use ::pgp::packet::{SignatureConfig, Subpacket, SubpacketData};
    use ::pgp::types::KeyDetails;
    let mut config =
        SignatureConfig::v4(typ, secret.primary_key.algorithm(), ::pgp::crypto::hash::HashAlgorithm::Sha256);
    let mut hashed =
        vec![Subpacket::regular(SubpacketData::SignatureCreationTime(::pgp::types::Timestamp::now())).unwrap()];
    if let Some(f) = flags {
        hashed.push(Subpacket::regular(SubpacketData::KeyFlags(f)).unwrap());
    }
    if let Some(secs) = expires_after {
        let d = ::pgp::types::Duration::try_from(std::time::Duration::from_secs(secs as u64)).unwrap();
        hashed.push(Subpacket::regular(SubpacketData::KeyExpirationTime(d)).unwrap());
    }
    hashed.push(Subpacket::regular(SubpacketData::IssuerFingerprint(secret.primary_key.fingerprint())).unwrap());
    config.hashed_subpackets = hashed;
    config
        .sign_subkey_binding(
            &secret.primary_key,
            secret.primary_key.public_key(),
            &::pgp::types::Password::empty(),
            sub,
        )
        .unwrap()
}

/// A key with two encryption subkeys.
fn two_encryption_subkeys() -> pgp::SignedSecretKey {
    use ::pgp::composed::{EncryptionCaps, KeyType, SecretKeyParamsBuilder, SubkeyParamsBuilder};
    use ::pgp::crypto::ecc_curve::ECCCurve;
    let sub = || {
        let mut enc = SubkeyParamsBuilder::default();
        enc.key_type(KeyType::ECDH(ECCCurve::Curve25519Legacy)).can_sign(false).can_encrypt(EncryptionCaps::All);
        enc.build().unwrap()
    };
    let mut params = SecretKeyParamsBuilder::default();
    params
        .key_type(KeyType::Ed25519Legacy)
        .can_certify(true)
        .can_sign(true)
        .can_encrypt(EncryptionCaps::None)
        .primary_user_id("<maria@example.org>".into())
        .subkeys(vec![sub(), sub()]);
    params.build().unwrap().generate(rand::thread_rng()).unwrap()
}

#[test]
fn expired_and_revoked_encryption_subkeys_are_skipped() {
    use ::pgp::packet::{KeyFlags, SignatureType};
    use ::pgp::types::KeyDetails;
    let secret = two_encryption_subkeys();
    let mut public = secret.to_public_key();
    let mut enc = KeyFlags::default();
    enc.set_encrypt_comms(true);
    enc.set_encrypt_storage(true);
    // The first subkey expires a day after it was made.
    let first = public.public_subkeys[0].key.clone();
    public.public_subkeys[0].signatures =
        vec![bind(&secret, &first, SignatureType::SubkeyBinding, Some(enc.clone()), Some(86_400))];
    let second_fp = public.public_subkeys[1].key.fingerprint();
    let made = first.created_at().as_secs() as i64;
    let at = |k: &pgp::SignedPublicKey, t: i64| pgp::encryption_subkey_at(k, t).map(|s| s.key.fingerprint());
    assert_eq!(at(&public, made + 2 * 86_400), Some(second_fp.clone()), "never the expired one");
    // Both bound, the second revoked: the first, as long as it has not expired.
    public.public_subkeys[0].signatures =
        vec![bind(&secret, &first, SignatureType::SubkeyBinding, Some(enc.clone()), None)];
    let second = public.public_subkeys[1].key.clone();
    public.public_subkeys[1].signatures.push(bind(&secret, &second, SignatureType::SubkeyRevocation, None, None));
    let reread = pgp::read_public(pgp::armored_public(&public).unwrap().as_bytes()).unwrap();
    assert_eq!(at(&reread, made + 60), Some(first.fingerprint()));
    // Encrypted, the message opens with the key's secret part (the revoked subkey's secret is
    // there too; the point is which one the message names).
    let armored = pgp::encrypt(b"Content-Type: text/plain\r\n\r\nx\r\n", &[reread], None).unwrap();
    let (msg, _) = ::pgp::composed::Message::from_string(&armored).unwrap();
    let ::pgp::composed::Message::Encrypted { esk, .. } = &msg else { panic!() };
    let named: Vec<String> = esk
        .iter()
        .filter_map(|e| match e {
            ::pgp::composed::Esk::PublicKeyEncryptedSessionKey(p) => p.id().ok().map(|i| format!("{i:?}")),
            _ => None,
        })
        .collect();
    assert_eq!(named, [format!("{:?}", first.legacy_key_id())]);
}

#[test]
fn signatures_come_from_the_signing_subkey() {
    use ::pgp::composed::{EncryptionCaps, KeyType, SecretKeyParamsBuilder, SubkeyParamsBuilder};
    use ::pgp::crypto::ecc_curve::ECCCurve;
    use ::pgp::types::KeyDetails;
    // A primary key that only certifies, a subkey that signs, one that encrypts (as GnuPG makes
    // them with `--quick-add-key … sign`).
    let mut sign = SubkeyParamsBuilder::default();
    sign.key_type(KeyType::Ed25519Legacy).can_sign(true).can_encrypt(EncryptionCaps::None);
    let mut enc = SubkeyParamsBuilder::default();
    enc.key_type(KeyType::ECDH(ECCCurve::Curve25519Legacy)).can_sign(false).can_encrypt(EncryptionCaps::All);
    let mut params = SecretKeyParamsBuilder::default();
    params
        .key_type(KeyType::Ed25519Legacy)
        .can_certify(true)
        .can_sign(false)
        .can_encrypt(EncryptionCaps::None)
        .primary_user_id("Maria <maria@example.org>".into())
        .subkeys(vec![sign.build().unwrap(), enc.build().unwrap()]);
    let secret = params.build().unwrap().generate(rand::thread_rng()).unwrap();
    let sub_fp = secret.secret_subkeys[0].key.fingerprint();
    assert_eq!(pgp::signing_key_at(&secret, 1_900_000_000).fingerprint(), sub_fp);
    let sig = pgp::sign_detached(&pgp::Secret::plain(secret.clone()), b"Hallo").unwrap();
    use ::pgp::composed::Deserializable;
    let (parsed, _) = ::pgp::composed::DetachedSignature::from_string(&sig).unwrap();
    assert_eq!(parsed.signature.issuer_fingerprint().first().map(|f| (*f).clone()), Some(sub_fp));
    assert!(matches!(
        pgp::verify_detached(sig.as_bytes(), b"Hallo", &[secret.to_public_key()]),
        pgp::Verdict::Valid { .. }
    ));
}

#[test]
fn smime_oaep_with_sha256_opens() {
    use der::{Decode, Encode};
    use rsa::traits::PublicKeyParts;
    let ca = test_ca();
    let p12 = smime_identity(&ca, "jonas@example.org");
    let id = smime::read_p12(&p12, "geheim").unwrap();
    let content = b"Content-Type: text/plain\r\n\r\nGeheim\r\n";
    // Encrypted as usual (RSA PKCS#1 v1.5), then the key transport rewritten as RSAES-OAEP with
    // SHA-256 and MGF1-SHA-256, as Outlook and OpenSSL `-keyopt rsa_padding_mode:oaep` write it.
    let der_bytes = smime::encrypt(content, std::slice::from_ref(&id.cert)).unwrap();
    let ci = cms::content_info::ContentInfo::from_der(&der_bytes).unwrap();
    let mut ed = cms::enveloped_data::EnvelopedData::from_der(&ci.content.to_der().unwrap()).unwrap();
    let mut infos = ed.recip_infos.0.clone().into_vec();
    let cms::enveloped_data::RecipientInfo::Ktri(k) = &mut infos[0] else { panic!() };
    let cek = id.key.decrypt(rsa::Pkcs1v15Encrypt, k.enc_key.as_bytes()).unwrap();
    let public = rsa::RsaPublicKey::new(id.key.n().clone(), id.key.e().clone()).unwrap();
    let wrapped = public
        .encrypt(&mut rand::thread_rng(), rsa::Oaep::new_with_mgf_hash::<sha2::Sha256, sha2::Sha256>(), &cek)
        .unwrap();
    let params = rsa::pkcs1::RsaOaepParams::new::<sha2::Sha256>().to_der().unwrap();
    k.key_enc_alg = spki::AlgorithmIdentifierOwned {
        oid: const_oid::db::rfc5912::ID_RSAES_OAEP,
        parameters: Some(der::Any::from_der(&params).unwrap()),
    };
    k.enc_key = der::asn1::OctetString::new(wrapped).unwrap();
    ed.recip_infos = cms::enveloped_data::RecipientInfos(der::asn1::SetOfVec::try_from(infos).unwrap());
    let ci = cms::content_info::ContentInfo {
        content_type: const_oid::db::rfc5911::ID_ENVELOPED_DATA,
        content: der::Any::encode_from(&ed).unwrap(),
    };
    let opened = smime::decrypt(&ci.to_der().unwrap(), &[id]).unwrap();
    assert_eq!(opened, content);
}
