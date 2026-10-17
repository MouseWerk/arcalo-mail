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
    let armored = pgp::encrypt("Geheim: 1234".as_bytes(), &[jonas_pub], Some(&secret)).unwrap();
    let body = format!("Hallo Jonas,\n\n{armored}\n");
    let (text, p) = open_inline(&jonas, &body, Some("maria@example.org"), &jonas_keys).unwrap();
    assert_eq!((text.as_str(), p.decrypted, p.signature.as_deref()), ("Geheim: 1234", true, Some("valid")));
    // A cleartext signature.
    let signed = ::pgp::composed::CleartextSignedMessage::sign(
        rand::thread_rng(),
        "Bitte bestätigen.\n",
        &secret.primary_key,
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
    let info = save_cert(s, &id.cert, true).unwrap();
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
    assert_eq!(certs_for(&jonas, "maria@example.org").unwrap().len(), 1, "the sender's certificate is kept");
    smime::trust_for_tests(ca.0.der().to_vec());
    let o = open(&jonas, &signed, Some("maria@example.org"), &jonas_keys).unwrap();
    assert_eq!(o.protection.signature.as_deref(), Some("valid"));
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
    save_cert(&maria, &certs_for(&jonas, "jonas@example.org").unwrap()[0], false).unwrap();
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
