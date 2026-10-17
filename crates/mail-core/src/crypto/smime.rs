//! S/MIME (RFC 8551) with the RustCrypto CMS types: the user's certificate and key from a
//! PKCS#12 file (kept in the credential store), detached signatures (`application/pkcs7-signature`),
//! encryption to the recipients' certificates (`enveloped-data`, RSA key transport, AES-256-CBC),
//! decryption, and verification: the signature over the signed attributes, the message digest,
//! and the certificate's chain to the system's root certificates (`emailProtection`). Recipients'
//! certificates come from their signed mail. RSA certificates only.

use der::asn1::OctetString;
use der::{Any, Decode, Encode};
use serde::Serialize;
use sha2::Digest;
use x509_cert::Certificate;

use cms::builder::{
    ContentEncryptionAlgorithm, EnvelopedDataBuilder, KeyEncryptionInfo, KeyTransRecipientInfoBuilder,
    SignedDataBuilder, SignerInfoBuilder, create_signing_time_attribute,
};
use cms::cert::{CertificateChoices, IssuerAndSerialNumber};
use cms::content_info::ContentInfo;
use cms::enveloped_data::{EnvelopedData, RecipientIdentifier, RecipientInfo};
use cms::signed_data::{EncapsulatedContentInfo, SignedData, SignerIdentifier};
use const_oid::db::{rfc5911, rfc5912};
use spki::AlgorithmIdentifierOwned;

use crate::error::{Error, Result};
use crate::{tr, trf};

fn cms_error(e: impl std::fmt::Display) -> Error {
    Error::Invalid(trf!("S/MIME: {}", "S/MIME: {}", e))
}

/// `1.3.6.1.5.5.7.3.4`, emailProtection, DER-encoded.
const EMAIL_PROTECTION: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x04];

/// A certificate as the settings and the reading pane show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CertInfo {
    /// SHA-256 of the DER, upper-case hex.
    pub fingerprint: String,
    pub subject: String,
    pub issuer: String,
    pub emails: Vec<String>,
    /// Unix seconds.
    pub not_after: i64,
}

pub fn fingerprint(der: &[u8]) -> String {
    sha2::Sha256::digest(der).iter().map(|b| format!("{b:02X}")).collect()
}

/// The common name of a distinguished name, else all of it.
fn common_name(name: &x509_cert::name::Name) -> String {
    for rdn in name.0.iter() {
        for atv in rdn.0.iter() {
            if atv.oid == const_oid::db::rfc4519::CN
                && let Ok(s) = atv
                    .value
                    .decode_as::<der::asn1::Utf8StringRef>()
                    .map(|s| s.to_string())
                    .or_else(|_| atv.value.decode_as::<der::asn1::PrintableStringRef>().map(|s| s.to_string()))
            {
                return s;
            }
        }
    }
    name.to_string()
}

/// The addresses a certificate is for: its `rfc822Name`s, else the subject's `emailAddress`.
fn emails_of(cert: &Certificate) -> Vec<String> {
    let mut out = vec![];
    for ext in cert.tbs_certificate.extensions.iter().flatten() {
        if ext.extn_id == const_oid::db::rfc5280::ID_CE_SUBJECT_ALT_NAME
            && let Ok(san) = x509_cert::ext::pkix::SubjectAltName::from_der(ext.extn_value.as_bytes())
        {
            for g in san.0 {
                if let x509_cert::ext::pkix::name::GeneralName::Rfc822Name(m) = g {
                    out.push(m.to_string().to_ascii_lowercase());
                }
            }
        }
    }
    if out.is_empty() {
        for rdn in cert.tbs_certificate.subject.0.iter() {
            for atv in rdn.0.iter() {
                if atv.oid == const_oid::db::rfc3280::EMAIL_ADDRESS
                    && let Ok(s) = atv.value.decode_as::<der::asn1::Ia5StringRef>()
                {
                    out.push(s.to_string().to_ascii_lowercase());
                }
            }
        }
    }
    out
}

pub fn cert_info(der_bytes: &[u8]) -> Result<CertInfo> {
    let cert = Certificate::from_der(der_bytes).map_err(cms_error)?;
    Ok(CertInfo {
        fingerprint: fingerprint(der_bytes),
        subject: common_name(&cert.tbs_certificate.subject),
        issuer: common_name(&cert.tbs_certificate.issuer),
        emails: emails_of(&cert),
        not_after: cert.tbs_certificate.validity.not_after.to_unix_duration().as_secs() as i64,
    })
}

/// The user's certificate with its key and the chain above it.
pub struct Identity {
    pub key: rsa::RsaPrivateKey,
    pub cert: Vec<u8>,
    pub chain: Vec<Vec<u8>>,
}

/// Reads a PKCS#12 file (`.p12`, `.pfx`) with its password.
pub fn read_p12(data: &[u8], password: &str) -> Result<Identity> {
    use rsa::pkcs8::DecodePrivateKey;
    let store = p12_keystore::KeyStore::from_pkcs12(data, password).map_err(|_| {
        Error::Invalid(
            tr!(
                "Die Datei lässt sich nicht öffnen. Stimmt das Passwort?",
                "The file cannot be opened. Is the password right?"
            )
            .into(),
        )
    })?;
    let (_, chain) = store
        .private_key_chain()
        .ok_or_else(|| Error::Invalid(tr!("Die Datei enthält keinen Schlüssel.", "The file holds no key.").into()))?;
    let key = rsa::RsaPrivateKey::from_pkcs8_der(chain.key()).map_err(|_| {
        Error::Invalid(tr!("Nur RSA-Zertifikate werden unterstützt.", "Only RSA certificates are supported.").into())
    })?;
    let certs: Vec<Vec<u8>> = chain.chain().iter().map(|c| c.as_der().to_vec()).collect();
    // The certificate of the key comes first; find it by its public key to be sure.
    let public = rsa::RsaPublicKey::from(&key);
    let own = certs.iter().position(|c| public_key(c).is_ok_and(|p| p == public)).ok_or_else(|| {
        Error::Invalid(tr!("Zum Schlüssel fehlt das Zertifikat.", "The key's certificate is missing.").into())
    })?;
    let cert = certs[own].clone();
    let chain = certs.into_iter().enumerate().filter(|(i, _)| *i != own).map(|(_, c)| c).collect();
    Ok(Identity { key, cert, chain })
}

fn public_key(cert_der: &[u8]) -> Result<rsa::RsaPublicKey> {
    use rsa::pkcs8::DecodePublicKey;
    let cert = Certificate::from_der(cert_der).map_err(cms_error)?;
    let spki = cert.tbs_certificate.subject_public_key_info.to_der().map_err(cms_error)?;
    rsa::RsaPublicKey::from_public_key_der(&spki).map_err(|_| {
        Error::Invalid(tr!("Nur RSA-Zertifikate werden unterstützt.", "Only RSA certificates are supported.").into())
    })
}

fn issuer_serial(cert_der: &[u8]) -> Result<IssuerAndSerialNumber> {
    let cert = Certificate::from_der(cert_der).map_err(cms_error)?;
    Ok(IssuerAndSerialNumber {
        issuer: cert.tbs_certificate.issuer.clone(),
        serial_number: cert.tbs_certificate.serial_number.clone(),
    })
}

/// A detached signature (DER `ContentInfo` with `SignedData`) over `data`.
pub fn sign_detached(id: &Identity, data: &[u8]) -> Result<Vec<u8>> {
    use rsa::pkcs1v15::SigningKey;
    let content = EncapsulatedContentInfo { econtent_type: rfc5911::ID_DATA, econtent: None };
    let digest = sha2::Sha256::digest(data).to_vec();
    let signer = SigningKey::<sha2::Sha256>::new(id.key.clone());
    let alg = AlgorithmIdentifierOwned { oid: rfc5912::ID_SHA_256, parameters: None };
    let mut info = SignerInfoBuilder::new(
        &signer,
        SignerIdentifier::IssuerAndSerialNumber(issuer_serial(&id.cert)?),
        alg.clone(),
        &content,
        Some(&digest),
    )
    .map_err(cms_error)?;
    info.add_signed_attribute(create_signing_time_attribute().map_err(cms_error)?).map_err(cms_error)?;
    let mut b = SignedDataBuilder::new(&content);
    b.add_digest_algorithm(alg).map_err(cms_error)?;
    for c in std::iter::once(&id.cert).chain(id.chain.iter()) {
        let cert = Certificate::from_der(c).map_err(cms_error)?;
        b.add_certificate(CertificateChoices::Certificate(cert)).map_err(cms_error)?;
    }
    b.add_signer_info::<SigningKey<sha2::Sha256>, rsa::pkcs1v15::Signature>(info).map_err(cms_error)?;
    b.build().map_err(cms_error)?.to_der().map_err(cms_error)
}

/// Encrypts `data` to the certificates (DER `ContentInfo` with `EnvelopedData`).
pub fn encrypt(data: &[u8], recipients: &[Vec<u8>]) -> Result<Vec<u8>> {
    let mut rng = rand::thread_rng();
    let mut infos = vec![];
    for r in recipients {
        infos.push((RecipientIdentifier::IssuerAndSerialNumber(issuer_serial(r)?), public_key(r)?));
    }
    // Each builder borrows a generator; one generator per recipient.
    let mut rngs: Vec<rand::rngs::ThreadRng> = infos.iter().map(|_| rand::thread_rng()).collect();
    let mut b =
        EnvelopedDataBuilder::new(None, data, ContentEncryptionAlgorithm::Aes256Cbc, None).map_err(cms_error)?;
    for ((rid, key), rng) in infos.into_iter().zip(rngs.iter_mut()) {
        b.add_recipient_info(
            KeyTransRecipientInfoBuilder::new(rid, KeyEncryptionInfo::Rsa(key), rng).map_err(cms_error)?,
        )
        .map_err(cms_error)?;
    }
    let ed = b.build_with_rng(&mut rng).map_err(cms_error)?;
    let ci =
        ContentInfo { content_type: rfc5911::ID_ENVELOPED_DATA, content: Any::encode_from(&ed).map_err(cms_error)? };
    ci.to_der().map_err(cms_error)
}

fn content_of(der_bytes: &[u8], expected: const_oid::ObjectIdentifier) -> Result<Vec<u8>> {
    let ci = ContentInfo::from_der(der_bytes).map_err(cms_error)?;
    if ci.content_type != expected {
        return Err(cms_error("unexpected content type"));
    }
    ci.content.to_der().map_err(cms_error)
}

/// Decrypts an `EnvelopedData` with one of the user's identities.
pub fn decrypt(der_bytes: &[u8], ids: &[Identity]) -> Result<Vec<u8>> {
    use aes::cipher::block_padding::Pkcs7;
    use aes::cipher::{BlockDecryptMut, KeyIvInit};
    let not_for_me = || {
        Error::Invalid(
            tr!(
                "Diese Nachricht ist nicht für eines deiner Zertifikate verschlüsselt.",
                "This message is not encrypted for one of your certificates."
            )
            .into(),
        )
    };
    let ed = EnvelopedData::from_der(&content_of(der_bytes, rfc5911::ID_ENVELOPED_DATA)?).map_err(cms_error)?;
    let mut cek = None;
    'outer: for ri in ed.recip_infos.0.iter() {
        let RecipientInfo::Ktri(k) = ri else { continue };
        for id in ids {
            let mine = match &k.rid {
                RecipientIdentifier::IssuerAndSerialNumber(is) => issuer_serial(&id.cert).is_ok_and(|own| own == *is),
                RecipientIdentifier::SubjectKeyIdentifier(_) => true,
            };
            if !mine {
                continue;
            }
            let plain = if k.key_enc_alg.oid == rfc5912::ID_RSAES_OAEP {
                id.key.decrypt(rsa::Oaep::new::<sha1::Sha1>(), k.enc_key.as_bytes())
            } else {
                id.key.decrypt(rsa::Pkcs1v15Encrypt, k.enc_key.as_bytes())
            };
            if let Ok(p) = plain {
                cek = Some(p);
                break 'outer;
            }
        }
    }
    let cek = cek.ok_or_else(not_for_me)?;
    let eci = &ed.encrypted_content;
    let iv = eci
        .content_enc_alg
        .parameters
        .as_ref()
        .and_then(|p| p.decode_as::<OctetString>().ok())
        .ok_or_else(|| cms_error("no IV"))?;
    let data = eci.encrypted_content.as_ref().ok_or_else(|| cms_error("no content"))?.as_bytes();
    let iv = iv.as_bytes();
    let bad = || cms_error("decryption failed");
    let out = match eci.content_enc_alg.oid {
        o if o == rfc5911::ID_AES_128_CBC => cbc::Decryptor::<aes::Aes128>::new_from_slices(&cek, iv)
            .map_err(|_| bad())?
            .decrypt_padded_vec_mut::<Pkcs7>(data),
        o if o == rfc5911::ID_AES_192_CBC => cbc::Decryptor::<aes::Aes192>::new_from_slices(&cek, iv)
            .map_err(|_| bad())?
            .decrypt_padded_vec_mut::<Pkcs7>(data),
        o if o == rfc5911::ID_AES_256_CBC => cbc::Decryptor::<aes::Aes256>::new_from_slices(&cek, iv)
            .map_err(|_| bad())?
            .decrypt_padded_vec_mut::<Pkcs7>(data),
        _ => {
            return Err(Error::Invalid(
                tr!(
                    "Diese Verschlüsselung wird nicht unterstützt (nur AES).",
                    "This encryption is not supported (AES only)."
                )
                .into(),
            ));
        }
    };
    out.map_err(|_| bad())
}

/// What a verification found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The signature is good; `trusted`: its certificate leads to a root this device trusts.
    Valid {
        signer: String,
        fingerprint: String,
        emails: Vec<String>,
        trusted: bool,
        cert: Vec<u8>,
    },
    Invalid,
}

fn verify_rsa(key: &rsa::RsaPublicKey, digest_oid: &const_oid::ObjectIdentifier, data: &[u8], sig: &[u8]) -> bool {
    use rsa::signature::Verifier;
    let Ok(sig) = rsa::pkcs1v15::Signature::try_from(sig) else { return false };
    match *digest_oid {
        o if o == rfc5912::ID_SHA_256 => {
            rsa::pkcs1v15::VerifyingKey::<sha2::Sha256>::new(key.clone()).verify(data, &sig).is_ok()
        }
        o if o == rfc5912::ID_SHA_384 => {
            rsa::pkcs1v15::VerifyingKey::<sha2::Sha384>::new(key.clone()).verify(data, &sig).is_ok()
        }
        o if o == rfc5912::ID_SHA_512 => {
            rsa::pkcs1v15::VerifyingKey::<sha2::Sha512>::new(key.clone()).verify(data, &sig).is_ok()
        }
        o if o == rfc5912::ID_SHA_1 => {
            rsa::pkcs1v15::VerifyingKey::<sha1::Sha1>::new(key.clone()).verify(data, &sig).is_ok()
        }
        _ => false,
    }
}

fn digest(oid: &const_oid::ObjectIdentifier, data: &[u8]) -> Option<Vec<u8>> {
    Some(match *oid {
        o if o == rfc5912::ID_SHA_256 => sha2::Sha256::digest(data).to_vec(),
        o if o == rfc5912::ID_SHA_384 => sha2::Sha384::digest(data).to_vec(),
        o if o == rfc5912::ID_SHA_512 => sha2::Sha512::digest(data).to_vec(),
        o if o == rfc5912::ID_SHA_1 => sha1::Sha1::digest(data).to_vec(),
        _ => return None,
    })
}

/// Whether a certificate chains to one of `anchors` (DER) for e-mail at `now` (Unix seconds).
pub fn trusted(cert: &[u8], intermediates: &[Vec<u8>], anchors: &[Vec<u8>], now: i64) -> bool {
    use rustls::pki_types::{CertificateDer, UnixTime};
    let ee_der = CertificateDer::from(cert.to_vec());
    let Ok(ee) = webpki::EndEntityCert::try_from(&ee_der) else { return false };
    let anchor_ders: Vec<CertificateDer> = anchors.iter().map(|a| CertificateDer::from(a.clone())).collect();
    let anchors: Vec<_> = anchor_ders.iter().filter_map(|a| webpki::anchor_from_trusted_cert(a).ok()).collect();
    let inter: Vec<CertificateDer> = intermediates.iter().map(|c| CertificateDer::from(c.clone())).collect();
    let algs = rustls::crypto::aws_lc_rs::default_provider().signature_verification_algorithms.all;
    ee.verify_for_usage(
        algs,
        &anchors,
        &inter,
        UnixTime::since_unix_epoch(std::time::Duration::from_secs(now.max(0) as u64)),
        webpki::KeyUsage::required_if_present(EMAIL_PROTECTION),
        None,
        None,
    )
    .is_ok()
}

/// Checks a `SignedData`: detached over `content`, or with its content inside (returned).
pub fn verify(
    der_bytes: &[u8],
    content: Option<&[u8]>,
    anchors: &[Vec<u8>],
    now: i64,
) -> Result<(Verdict, Option<Vec<u8>>)> {
    let sd = SignedData::from_der(&content_of(der_bytes, rfc5911::ID_SIGNED_DATA)?).map_err(cms_error)?;
    let inside = sd
        .encap_content_info
        .econtent
        .as_ref()
        .and_then(|e| e.decode_as::<OctetString>().ok())
        .map(|o| o.as_bytes().to_vec());
    let data: Vec<u8> = match (content, &inside) {
        (Some(c), _) => c.to_vec(),
        (None, Some(i)) => i.clone(),
        (None, None) => return Err(cms_error("no content")),
    };
    let certs: Vec<Vec<u8>> = sd
        .certificates
        .iter()
        .flat_map(|set| set.0.iter())
        .filter_map(|c| match c {
            CertificateChoices::Certificate(c) => c.to_der().ok(),
            _ => None,
        })
        .collect();
    for si in sd.signer_infos.0.iter() {
        let SignerIdentifier::IssuerAndSerialNumber(is) = &si.sid else { continue };
        let Some(cert) = certs.iter().find(|c| issuer_serial(c).is_ok_and(|x| x == *is)) else { continue };
        let Ok(key) = public_key(cert) else { continue };
        let Some(hash) = digest(&si.digest_alg.oid, &data) else { continue };
        let ok = match &si.signed_attrs {
            Some(attrs) => {
                let md = attrs
                    .iter()
                    .find(|a| a.oid == rfc5911::ID_MESSAGE_DIGEST)
                    .and_then(|a| a.values.get(0))
                    .and_then(|v| v.decode_as::<OctetString>().ok());
                let matches = md.is_some_and(|m| m.as_bytes() == hash.as_slice());
                let Ok(signed) = attrs.to_der() else { continue };
                matches && verify_rsa(&key, &si.digest_alg.oid, &signed, si.signature.as_bytes())
            }
            None => verify_rsa(&key, &si.digest_alg.oid, &data, si.signature.as_bytes()),
        };
        if !ok {
            return Ok((Verdict::Invalid, inside));
        }
        let info = cert_info(cert)?;
        let others: Vec<Vec<u8>> = certs.iter().filter(|c| *c != cert).cloned().collect();
        return Ok((
            Verdict::Valid {
                signer: info.subject,
                fingerprint: info.fingerprint,
                emails: info.emails,
                trusted: trusted(cert, &others, anchors, now),
                cert: cert.clone(),
            },
            inside,
        ));
    }
    Ok((Verdict::Invalid, inside))
}

/// The root certificates this device trusts: the system's store (Windows, macOS, Linux), Mozilla's
/// list on the phones; read once.
pub fn system_anchors() -> Vec<Vec<u8>> {
    static ROOTS: std::sync::OnceLock<Vec<Vec<u8>>> = std::sync::OnceLock::new();
    let mut out = ROOTS
        .get_or_init(|| {
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            {
                rustls_native_certs::load_native_certs().certs.into_iter().map(|c| c.to_vec()).collect()
            }
            #[cfg(any(target_os = "android", target_os = "ios"))]
            {
                webpki_root_certs::TLS_SERVER_ROOT_CERTS.iter().map(|c| c.to_vec()).collect()
            }
        })
        .clone();
    out.extend(EXTRA.lock().unwrap_or_else(|e| e.into_inner()).iter().cloned());
    out
}

/// Root certificates trusted besides the system's (the tests' own authority).
static EXTRA: std::sync::Mutex<Vec<Vec<u8>>> = std::sync::Mutex::new(Vec::new());

#[cfg(any(test, feature = "fake"))]
pub fn trust_for_tests(der: Vec<u8>) {
    EXTRA.lock().unwrap_or_else(|e| e.into_inner()).push(der);
}
