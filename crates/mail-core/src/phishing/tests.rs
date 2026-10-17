use std::path::Path;

use super::*;
use crate::account::{AccountDraft, Provider, preset};
use crate::imap::MailboxEntry;
use crate::parse::parse;
use crate::store::tests::header;

/// The kinds of reasons, as the corpus files name them in `X-Expect`.
fn kinds(reasons: &[Reason]) -> Vec<String> {
    let mut k: Vec<String> = reasons
        .iter()
        .map(|r| serde_json::to_value(r).unwrap()["reason"].as_str().unwrap_or_default().to_string())
        .collect();
    k.sort();
    k
}

fn header_value(raw: &str, name: &str) -> String {
    raw.lines().find_map(|l| l.strip_prefix(&format!("{name}: "))).unwrap_or_default().trim().to_string()
}

/// The people the corpus's user wrote to.
fn corpus_context(first: bool, dmarc_pass: bool) -> Context {
    Context {
        correspondents: vec!["kanzlei-schmidt.de".into(), "mueller-bau.de".into(), "verein-example.de".into()],
        written_to: ["anna@mueller-bau.de", "dr.schmidt@kanzlei-schmidt.de", "kontakt@verein-example.de"]
            .into_iter()
            .map(String::from)
            .collect(),
        first_contact: first,
        dmarc_pass,
    }
}

/// Every example in `corpus/` (made up, no network): the reasons found are exactly the ones its
/// `X-Expect` header names (`none`: no warning). `X-Context: first` is a first contact.
#[test]
fn the_corpus_gets_exactly_the_expected_reasons() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/phishing/corpus");
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).collect();
    files.sort();
    assert!(files.len() >= 20, "corpus of {} files", files.len());
    let own = vec!["example.org".to_string()];
    let mut failures = vec![];
    for f in &files {
        let raw = std::fs::read_to_string(f).unwrap();
        let p = parse(raw.as_bytes()).unwrap();
        let from = p.from.clone().unwrap();
        let auth = p.list.authentication.as_deref().map(|a| auth_failures(a, &from.email, &own).join(" "));
        let dmarc_pass =
            p.list.authentication.as_deref().is_some_and(|a| crate::avatars::dmarc_passed(a, &from.email, &own));
        let input = Input {
            from: Some(&from),
            reply_to: &p.reply_to,
            subject: &p.subject,
            list_id: p.list.list_id.as_deref(),
            auth_fail: auth.as_deref(),
            html: p.html.as_deref(),
        };
        let ctx = corpus_context(header_value(&raw, "X-Context") == "first", dmarc_pass);
        let got = kinds(&analyze(&input, &ctx));
        let mut want: Vec<String> =
            header_value(&raw, "X-Expect").split_whitespace().filter(|k| *k != "none").map(String::from).collect();
        want.sort();
        if got != want {
            failures.push(format!("{}: want {want:?}, got {got:?}", f.file_name().unwrap().to_string_lossy()));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn skeletons_registrable_domains_and_distances() {
    assert_eq!(skeleton("paypa1"), skeleton("paypal"));
    assert_eq!(skeleton("rnicrosoft"), skeleton("microsoft"));
    assert_eq!(skeleton("\u{0430}pple"), skeleton("apple"));
    assert_eq!(skeleton("g00gle"), skeleton("google"));
    assert_ne!(skeleton("posteo"), skeleton("postbank"));
    assert_eq!(registrable("mail.service.paypal.co.uk"), "paypal.co.uk");
    assert_eq!(registrable("login.sparkasse.de."), "sparkasse.de");
    assert_eq!(registrable("example.org"), "example.org");
    assert_eq!(distance("sparkase", "sparkasse"), 1);
    assert_eq!(distance("paypla", "paypal"), 1, "a swap is one");
    assert!(distance("amazon", "netflix") > 2);
    assert_eq!(unicode_domain("xn--pple-43d.com"), "\u{0430}pple.com");
    assert!(mixed_script("\u{0430}pple"));
    assert!(mixed_script("\u{0440}\u{0430}\u{0443}\u{0440}\u{0430}l"));
    assert!(!mixed_script("m\u{00fc}ller"), "umlauts are Latin");
    assert!(!mixed_script("\u{043f}\u{0440}\u{0438}\u{043c}\u{0435}\u{0440}"), "a Russian word is no look-alike");
    assert!(has_word("ihr dhl paket", "dhl"));
    assert!(!has_word("bitte update", "dpd"));
    assert_eq!(address_in("\"service@paypal.de\""), Some("service@paypal.de".into()));
    assert_eq!(domain_in("amazon.de Kundendienst"), Some("amazon.de".into()));
    assert_eq!(domain_in("Dr. Schmidt"), None);
    assert_eq!(domain_in("Version 1.2"), None);
}

#[test]
fn links_follow_redirects_and_skip_mailing_services() {
    let html = r#"<a href="https://eur01.safelinks.protection.outlook.com/?url=https%3A%2F%2Fevil.example.ru%2Flogin">www.paypal.com</a>
        <a href="https://news.us1.list-manage.com/track/click?u=1">www.verein-example.de</a>
        <a href="https://evil.example.ru/2">www.paypal.com</a>
        <a href="mailto:x@paypal.com">paypal.com</a>
        <a href="https://www.paypal.com/de">Zu PayPal</a>"#;
    let found = mismatched_links(html, "sender.example");
    assert_eq!(found, vec![("paypal.com".to_string(), "example.ru".to_string())], "the same pair once");
}

#[test]
fn only_the_users_own_provider_counts_for_dmarc_and_spf() {
    let own = vec!["example.org".to_string()];
    let ar = "mx.example.org; spf=fail smtp.mailfrom=bank.example; dkim=fail; dmarc=fail (p=REJECT) header.from=bank.example";
    let mut got = auth_failures(ar, "info@bank.example", &own);
    got.sort();
    assert_eq!(got, ["dmarc", "spf"]);
    assert!(auth_failures(ar, "info@other.example", &own).contains(&"spf"), "SPF is about the envelope");
    assert!(!auth_failures(ar, "info@other.example", &own).contains(&"dmarc"), "DMARC for the From's domain only");
    assert!(auth_failures(&ar.replace("mx.example.org", "mx.evil.example"), "info@bank.example", &own).is_empty());
    assert!(
        auth_failures(
            &ar.replace("spf=fail", "spf=softfail").replace("dmarc=fail", "dmarc=pass"),
            "info@bank.example",
            &own
        )
        .is_empty()
    );
}

fn store_with(account_email: &str) -> (Store, i64) {
    let s = Store::open_in_memory().unwrap();
    let account = s
        .add_account(&AccountDraft {
            provider: Provider::Imap,
            name: "Privat".into(),
            email: account_email.into(),
            sender_name: "Maria".into(),
            transport: preset(Provider::Imap, account_email).unwrap(),
        })
        .unwrap();
    let inbox = s
        .upsert_mailbox(account, &MailboxEntry { name: "INBOX".into(), delimiter: Some('/'), attributes: vec![] })
        .unwrap();
    (s, inbox)
}

fn deliver(s: &Store, inbox: i64, uid: u32, raw: &str) -> i64 {
    let p = parse(raw.as_bytes()).unwrap();
    let id = s.insert_message(inbox, &header(uid, raw.as_bytes()), &p).unwrap();
    s.set_body_raw(id, &p, Some(raw.as_bytes())).unwrap();
    id
}

#[test]
fn the_store_knows_first_contacts_and_senders_the_user_trusts() {
    let (s, inbox) = store_with("maria@example.org");
    let spoof = "From: Sparkasse Service <service@sparkase.de>\r\nTo: maria@example.org\r\nSubject: Sicherheit\r\nMessage-ID: <a@x>\r\nDate: Mon, 6 Oct 2025 10:00:00 +0200\r\nAuthentication-Results: imap.example.org; spf=fail smtp.mailfrom=sparkase.de; dmarc=fail header.from=sparkase.de\r\n\r\nBitte anmelden.\r\n";
    let id = deliver(&s, inbox, 1, spoof);
    let w = check(&s, id, None).unwrap().expect("a warning");
    assert_eq!(w.sender, "service@sparkase.de");
    assert_eq!(kinds(&w.reasons), ["auth_failed", "lookalike"]);
    assert!(w.reasons.contains(&Reason::AuthFailed { checks: vec!["spf".into(), "dmarc".into()] }));
    // „Das ist in Ordnung“: no warning for this sender any more; and back.
    set_ok(&s, "Service@Sparkase.de", true).unwrap();
    assert!(check(&s, id, None).unwrap().is_none());
    set_ok(&s, "service@sparkase.de", false).unwrap();
    assert!(check(&s, id, None).unwrap().is_some());
    // The setting switches the warning off.
    let mut settings = crate::settings::load(&s).unwrap();
    settings.phishing_warning = false;
    crate::settings::save(&s, &settings).unwrap();
    assert!(check(&s, id, None).unwrap().is_none());
    settings.phishing_warning = true;
    crate::settings::save(&s, &settings).unwrap();

    // A first contact claiming to be a parcel service; the second mail of the same sender is no
    // first contact any more.
    let parcel = |n: u32| {
        format!(
            "From: Paketdienst <info@zustell-info.example>\r\nTo: maria@example.org\r\nSubject: Ihr DHL Paket\r\nMessage-ID: <p{n}@x>\r\nDate: Mon, {n} Oct 2025 10:00:00 +0200\r\n\r\nZoll bezahlen.\r\n"
        )
    };
    let first = deliver(&s, inbox, 2, &parcel(6));
    assert_eq!(kinds(&check(&s, first, None).unwrap().unwrap().reasons), ["first_contact"]);
    let second = deliver(&s, inbox, 3, &parcel(7));
    assert!(check(&s, second, None).unwrap().is_none(), "not the first mail of this sender");

    // A look-alike of someone the user wrote to.
    s.touch_contact(&Address { name: None, email: "anna@mueller-bau.de".into() }, true, 1).unwrap();
    let fake = "From: Anna <anna@muller-bau.de>\r\nTo: maria@example.org\r\nSubject: Konto\r\nMessage-ID: <f@x>\r\nDate: Mon, 6 Oct 2025 10:00:00 +0200\r\n\r\nNeue Bankverbindung.\r\n";
    let id = deliver(&s, inbox, 4, fake);
    assert_eq!(
        check(&s, id, None).unwrap().unwrap().reasons,
        vec![Reason::Lookalike { domain: "muller-bau.de".into(), like: "mueller-bau.de".into() }]
    );
    // The genuine one: nothing.
    let real = fake.replace("muller-bau", "mueller-bau").replace("<f@x>", "<g@x>");
    let id = deliver(&s, inbox, 5, &real);
    assert!(check(&s, id, None).unwrap().is_none());
    // Mail from the user's own address is never warned about.
    let own = spoof.replace("service@sparkase.de", "maria@example.org").replace("<a@x>", "<o@x>");
    let id = deliver(&s, inbox, 6, &own);
    assert!(check(&s, id, None).unwrap().is_none());
}
