//! The phishing warning: signs that a message is not from whom it claims to be, found on the
//! device from the header, the HTML and what the user's own provider wrote in
//! `Authentication-Results`. The reading pane shows a calm banner with the reasons; nothing is
//! blocked, moved or hidden, and „Das ist in Ordnung“ silences the warning for a sender.
//!
//! The signs ([`Reason`]):
//!
//! * a sender domain that looks like a known one: a brand's ([`BRANDS`]) or one of the user's
//!   correspondents' (people the user wrote to), by Unicode confusables (Cyrillic „а“ for „a“,
//!   „rn“ for „m“, „1“ for „l“), by punycode (`xn--`) that shows look-alike letters, or by one
//!   typed letter more, less or different;
//! * a reply address on another domain than the sender's (not for mailing lists, not for an
//!   address the user wrote to);
//! * a display name that contains an address or a domain other than the sender's;
//! * a first contact that calls itself a bank, a payment service or a parcel service (by name,
//!   address or subject) without writing from that company's domain;
//! * a link whose text shows another domain than the one it opens;
//! * DMARC or SPF failed, as the user's provider found (only its own `Authentication-Results`
//!   count, as for BIMI: [`crate::avatars::own_authservers`]).
//!
//! No network: the domain lists are compiled in, the correspondents come from the store.

use std::collections::HashSet;

use rusqlite::{OptionalExtension, params};
use serde::Serialize;

use crate::error::Result;
use crate::parse::Address;
use crate::store::{Store, now};

/// Why a message looks like phishing; the UI words each one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Reason {
    /// The sender's domain looks like `like` (a brand or a correspondent) but is not it.
    Lookalike { domain: String, like: String },
    /// The domain is written in punycode (`domain`) and shows look-alike letters (`shown`).
    Punycode { domain: String, shown: String },
    /// Answers go to `reply_to`, on another domain than the sender's.
    ReplyTo { reply_to: String, from: String },
    /// The display name shows `shown`, but the mail comes from `actual`.
    NameAddress { shown: String, actual: String },
    /// A first contact claims to be `claim` (a bank, a payment or a parcel service).
    FirstContact { claim: String, kind: ClaimKind },
    /// A link reads `shown` and opens `target`; `more`: how many other such links.
    Link { shown: String, target: String, more: usize },
    /// The provider found DMARC and/or SPF failing (`checks`: `dmarc`, `spf`).
    AuthFailed { checks: Vec<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimKind {
    Bank,
    Payment,
    Parcel,
    Company,
}

/// The warning of one message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Warning {
    /// The sender the warning is about (for „Das ist in Ordnung“).
    pub sender: String,
    pub reasons: Vec<Reason>,
}

/// A company people impersonate: its name as written in mail, the domains it sends from, what
/// kind of company it is.
pub struct Brand {
    pub name: &'static str,
    /// Words that name it in a display name, an address or a subject (lower case).
    pub tokens: &'static [&'static str],
    /// Its own domains (organizational): mail from these or their subdomains is genuine as far as
    /// the name goes.
    pub domains: &'static [&'static str],
    pub kind: ClaimKind,
}

/// The companies phishing mail most often claims to be, in Germany and beyond.
pub const BRANDS: &[Brand] = &[
    Brand {
        name: "PayPal",
        tokens: &["paypal"],
        domains: &["paypal.com", "paypal.de", "paypal.at", "paypal.ch", "paypal-communication.com"],
        kind: ClaimKind::Payment,
    },
    Brand { name: "Klarna", tokens: &["klarna"], domains: &["klarna.com", "klarna.de"], kind: ClaimKind::Payment },
    Brand {
        name: "Visa",
        tokens: &["visa card", "visa secure"],
        domains: &["visa.com", "visa.de"],
        kind: ClaimKind::Payment,
    },
    Brand {
        name: "Mastercard",
        tokens: &["mastercard"],
        domains: &["mastercard.com", "mastercard.de"],
        kind: ClaimKind::Payment,
    },
    Brand {
        name: "American Express",
        tokens: &["american express", "amex"],
        domains: &["americanexpress.com", "aexp.com", "americanexpress.de"],
        kind: ClaimKind::Payment,
    },
    Brand {
        name: "Sparkasse",
        tokens: &["sparkasse"],
        domains: &["sparkasse.de", "s-servicecenter.de", "starfinanz.de", "sparkasse.at"],
        kind: ClaimKind::Bank,
    },
    Brand {
        name: "Volksbank",
        tokens: &["volksbank", "raiffeisenbank", "vr-bank", "vr bank"],
        domains: &["vr.de", "volksbank.de", "atruvia.de", "fiduciagad.de"],
        kind: ClaimKind::Bank,
    },
    Brand {
        name: "Deutsche Bank",
        tokens: &["deutsche bank", "deutschebank"],
        domains: &["db.com", "deutsche-bank.de"],
        kind: ClaimKind::Bank,
    },
    Brand {
        name: "Commerzbank",
        tokens: &["commerzbank"],
        domains: &["commerzbank.de", "commerzbank.com"],
        kind: ClaimKind::Bank,
    },
    Brand { name: "Postbank", tokens: &["postbank"], domains: &["postbank.de"], kind: ClaimKind::Bank },
    Brand { name: "ING", tokens: &["ing-diba", "ing diba"], domains: &["ing.de", "ing.com"], kind: ClaimKind::Bank },
    Brand { name: "DKB", tokens: &["dkb"], domains: &["dkb.de"], kind: ClaimKind::Bank },
    Brand { name: "comdirect", tokens: &["comdirect"], domains: &["comdirect.de"], kind: ClaimKind::Bank },
    Brand { name: "N26", tokens: &["n26"], domains: &["n26.com"], kind: ClaimKind::Bank },
    Brand { name: "Consorsbank", tokens: &["consorsbank"], domains: &["consorsbank.de"], kind: ClaimKind::Bank },
    Brand { name: "Targobank", tokens: &["targobank"], domains: &["targobank.de"], kind: ClaimKind::Bank },
    Brand {
        name: "Santander",
        tokens: &["santander"],
        domains: &["santander.de", "santander.com"],
        kind: ClaimKind::Bank,
    },
    Brand {
        name: "DHL",
        tokens: &["dhl"],
        domains: &["dhl.de", "dhl.com", "deutschepost.de", "dpdhl.com"],
        kind: ClaimKind::Parcel,
    },
    Brand {
        name: "Deutsche Post",
        tokens: &["deutsche post"],
        domains: &["deutschepost.de", "dhl.de"],
        kind: ClaimKind::Parcel,
    },
    Brand {
        name: "Hermes",
        tokens: &["hermes"],
        domains: &["myhermes.de", "hermesworld.com", "hermes-europe.eu"],
        kind: ClaimKind::Parcel,
    },
    Brand { name: "DPD", tokens: &["dpd"], domains: &["dpd.de", "dpd.com"], kind: ClaimKind::Parcel },
    Brand {
        name: "GLS",
        tokens: &["gls"],
        domains: &["gls-pakete.de", "gls-group.eu", "gls-group.com"],
        kind: ClaimKind::Parcel,
    },
    Brand { name: "UPS", tokens: &["ups"], domains: &["ups.com"], kind: ClaimKind::Parcel },
    Brand { name: "FedEx", tokens: &["fedex"], domains: &["fedex.com"], kind: ClaimKind::Parcel },
    Brand {
        name: "Amazon",
        tokens: &["amazon"],
        domains: &[
            "amazon.de",
            "amazon.com",
            "amazon.co.uk",
            "amazon.at",
            "amazonses.com",
            "amazonaws.com",
            "amazon.fr",
            "amazon.it",
            "amazon.es",
        ],
        kind: ClaimKind::Company,
    },
    Brand {
        name: "Apple",
        tokens: &["apple id", "icloud"],
        domains: &["apple.com", "icloud.com", "me.com", "mac.com"],
        kind: ClaimKind::Company,
    },
    Brand {
        name: "Microsoft",
        tokens: &["microsoft", "outlook team", "office 365"],
        domains: &[
            "microsoft.com",
            "microsoftonline.com",
            "office.com",
            "outlook.com",
            "live.com",
            "hotmail.com",
            "office365.com",
            "msn.com",
        ],
        kind: ClaimKind::Company,
    },
    Brand {
        name: "Google",
        tokens: &["google"],
        domains: &["google.com", "gmail.com", "googlemail.com", "youtube.com", "google.de"],
        kind: ClaimKind::Company,
    },
    Brand { name: "Netflix", tokens: &["netflix"], domains: &["netflix.com"], kind: ClaimKind::Company },
    Brand {
        name: "eBay",
        tokens: &["ebay"],
        domains: &["ebay.de", "ebay.com", "kleinanzeigen.de"],
        kind: ClaimKind::Company,
    },
    Brand {
        name: "Telekom",
        tokens: &["telekom", "t-online"],
        domains: &["telekom.de", "t-online.de", "telekom.com"],
        kind: ClaimKind::Company,
    },
    Brand {
        name: "Vodafone",
        tokens: &["vodafone"],
        domains: &["vodafone.de", "vodafone.com"],
        kind: ClaimKind::Company,
    },
    Brand {
        name: "1&1",
        tokens: &["1&1", "1und1"],
        domains: &["1und1.de", "ionos.de", "ionos.com"],
        kind: ClaimKind::Company,
    },
    Brand {
        name: "GMX",
        tokens: &["gmx"],
        domains: &["gmx.de", "gmx.net", "gmx.at", "gmx.ch", "gmx.com"],
        kind: ClaimKind::Company,
    },
    Brand { name: "WEB.DE", tokens: &["web.de"], domains: &["web.de"], kind: ClaimKind::Company },
    Brand { name: "Elster", tokens: &["elster", "finanzamt"], domains: &["elster.de"], kind: ClaimKind::Company },
];

/// Words of a bank, a payment or a parcel service in a display name or an address; a first
/// contact using them is a claim of its own.
const CLAIM_WORDS: &[(&str, ClaimKind)] = &[
    ("bank", ClaimKind::Bank),
    ("banking", ClaimKind::Bank),
    ("kreditkarte", ClaimKind::Payment),
    ("credit card", ClaimKind::Payment),
    ("zahlung", ClaimKind::Payment),
    ("payment", ClaimKind::Payment),
    ("paket", ClaimKind::Parcel),
    ("sendung", ClaimKind::Parcel),
    ("zustellung", ClaimKind::Parcel),
    ("parcel", ClaimKind::Parcel),
    ("delivery", ClaimKind::Parcel),
    ("shipping", ClaimKind::Parcel),
];

/// Mail providers whose names are close to each other on purpose or by chance (`gmail`, `email`,
/// `gmx`): never taken for look-alikes of one another.
const PROVIDERS: &[&str] = &[
    "gmail.com",
    "googlemail.com",
    "email.de",
    "mail.de",
    "mail.com",
    "gmx.de",
    "gmx.net",
    "gmx.at",
    "gmx.ch",
    "web.de",
    "yahoo.com",
    "yahoo.de",
    "outlook.com",
    "outlook.de",
    "hotmail.com",
    "hotmail.de",
    "live.com",
    "live.de",
    "icloud.com",
    "me.com",
    "mac.com",
    "posteo.de",
    "posteo.net",
    "mailbox.org",
    "aol.com",
    "aol.de",
    "proton.me",
    "protonmail.com",
    "t-online.de",
    "freenet.de",
    "arcor.de",
    "online.de",
    "fastmail.com",
    "tutanota.com",
    "tuta.io",
];

/// Hosts of mailing services whose click-counting links stand between a link's text and its
/// address (not look-alikes of anything).
const CLICK_HOSTS: &[&str] = &[
    "list-manage.com",
    "mailchimp.com",
    "mcusercontent.com",
    "sendgrid.net",
    "mandrillapp.com",
    "sparkpostmail.com",
    "rs6.net",
    "createsend.com",
    "cmail19.com",
    "cmail20.com",
    "exacttarget.com",
    "hubspotlinks.com",
    "hs-sites.com",
    "mailgun.org",
    "mlsend.com",
    "brevo.com",
    "sendibt2.com",
    "sendinblue.com",
    "newsletter2go.com",
    "cleverreach.com",
    "rapidmail.de",
    "awstrack.me",
    "klclick.com",
    "klclick1.com",
    "ct.sendgrid.net",
];

/// Two-label public suffixes common in mail (a registrable domain under them has three labels).
const SECOND_LEVEL: &[&str] = &[
    "co.uk", "org.uk", "ac.uk", "gov.uk", "me.uk", "com.au", "net.au", "org.au", "co.nz", "co.jp", "ne.jp", "or.jp",
    "com.br", "com.tr", "co.za", "co.in", "com.mx", "com.cn", "gv.at", "or.at", "co.at", "com.pl", "com.es", "co.il",
];

/// The registrable (organizational) domain: the last two labels, three under a known two-label
/// suffix. Lower case, without a trailing dot.
pub fn registrable(domain: &str) -> String {
    let d = domain.trim().trim_end_matches('.').to_lowercase();
    let labels: Vec<&str> = d.split('.').filter(|l| !l.is_empty()).collect();
    if labels.len() <= 2 {
        return labels.join(".");
    }
    let last2 = labels[labels.len() - 2..].join(".");
    let take = if SECOND_LEVEL.contains(&last2.as_str()) { 3 } else { 2 };
    labels[labels.len().saturating_sub(take)..].join(".")
}

/// The name part of a registrable domain (`paypal` of `paypal.co.uk`).
fn label(registrable: &str) -> &str {
    registrable.split('.').next().unwrap_or(registrable)
}

fn same_or_sub(domain: &str, of: &str) -> bool {
    domain == of || domain.ends_with(&format!(".{of}"))
}

fn domain_of(email: &str) -> Option<String> {
    let d = email.rsplit_once('@')?.1.trim().trim_end_matches('>').to_lowercase();
    (d.contains('.') && !d.is_empty()).then_some(d)
}

/// A domain as people read it: punycode labels decoded.
pub fn unicode_domain(domain: &str) -> String {
    if !domain.split('.').any(|l| l.starts_with("xn--")) {
        return domain.to_lowercase();
    }
    let (shown, res) = idna::domain_to_unicode(domain);
    if res.is_ok() { shown } else { domain.to_lowercase() }
}

/// One character as the Latin letters it looks like (Unicode's confusables for the letters
/// phishing uses; diacritics dropped).
fn confusable(c: char) -> Option<&'static str> {
    Some(match c {
        // Cyrillic
        'а' => "a",
        'в' => "b",
        'е' | 'ё' => "e",
        'к' => "k",
        'м' => "m",
        'н' => "h",
        'о' => "o",
        'р' => "p",
        'с' => "c",
        'т' => "t",
        'у' => "y",
        'х' => "x",
        'ѕ' => "s",
        'і' | 'ї' => "i",
        'ј' => "j",
        'ԁ' => "d",
        'һ' => "h",
        'ӏ' => "l",
        'ԛ' => "q",
        'ԝ' => "w",
        'ɡ' => "g",
        'ո' => "n",
        'ս' => "u",
        // Greek
        'α' => "a",
        'β' => "b",
        'ε' => "e",
        'ι' => "i",
        'κ' => "k",
        'ν' => "v",
        'ο' => "o",
        'ρ' => "p",
        'τ' => "t",
        'υ' => "u",
        'χ' => "x",
        'ω' => "w",
        // Latin with marks, dotless and look-alike forms
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'ç' | 'ć' | 'č' => "c",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => "e",
        'ì' | 'í' | 'î' | 'ï' | 'ı' | 'ī' | 'į' => "i",
        'ł' | 'ľ' | 'ĺ' | 'ⅼ' => "l",
        'ñ' | 'ń' | 'ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => "o",
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' => "u",
        'ý' | 'ÿ' => "y",
        'ś' | 'š' | 'ş' => "s",
        'ź' | 'ż' | 'ž' => "z",
        'ß' => "ss",
        // Digits that pass for letters
        '0' => "o",
        '1' => "l",
        '3' => "e",
        '5' => "s",
        _ => return None,
    })
}

/// The skeleton of a domain label: every character as the letter it looks like, `rn` as `m`,
/// `vv` as `w`, `i` as `l` (they look alike in many fonts). Two labels with the same skeleton are
/// look-alikes.
pub fn skeleton(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    for c in label.to_lowercase().chars() {
        match confusable(c) {
            Some(s) => out.push_str(s),
            None => out.push(c),
        }
    }
    out.replace("rn", "m").replace("vv", "w").replace('i', "l").replace('-', "")
}

/// A label with Cyrillic or Greek letters that pass for Latin ones: mixed with Latin letters, or
/// made only of such look-alikes (`раураl`, `аррle`). German umlauts and other Latin letters with
/// marks are not counted here (a look-alike of a known name is still found by its skeleton).
fn mixed_script(label: &str) -> bool {
    let foreign = |c: char| ('\u{0370}'..='\u{03FF}').contains(&c) || ('\u{0400}'..='\u{052F}').contains(&c);
    if !label.chars().any(foreign) {
        return false;
    }
    let latin = label.chars().any(|c| c.is_ascii_alphabetic());
    let all_lookalike = label.chars().all(|c| !foreign(c) || confusable(c).is_some());
    latin || all_lookalike
}

/// Levenshtein distance with a cap (enough to tell 0, 1, 2 and more).
fn distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.len().abs_diff(b.len()) > 2 {
        return 3;
    }
    let mut before: Vec<usize> = vec![0; b.len() + 1];
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
            // A swap of two neighbours counts as one (paypla / paypal).
            if i > 0 && j > 0 && a[i] == b[j - 1] && a[i - 1] == b[j] {
                cur[j + 1] = cur[j + 1].min(before[j - 1] + 1);
            }
        }
        before = std::mem::replace(&mut prev, cur);
    }
    prev[b.len()]
}

/// The known domain the registrable domain `d` imitates, if any.
fn lookalike_of(d: &str, known: &[String]) -> Option<String> {
    let dl = label(d);
    let shown = unicode_domain(d);
    let sl = label(&shown);
    let skel = skeleton(sl);
    if PROVIDERS.contains(&d) || known.iter().any(|k| k.as_str() == d || same_or_sub(d, k)) {
        return None;
    }
    let mut best: Option<String> = None;
    for k in known {
        let kl = label(k);
        if kl == dl || kl.chars().count() < 4 {
            // The same name under another ending (paypal.de, paypal.com) or too short to tell.
            continue;
        }
        let similar = skeleton(kl) == skel || {
            let n = kl.chars().count();
            let limit = if n >= 10 {
                2
            } else if n >= 5 {
                1
            } else {
                0
            };
            limit > 0 && distance(sl, kl) <= limit
        };
        if similar {
            best = Some(k.clone());
            break;
        }
    }
    best
}

/// What the header of a message says, for [`analyze`].
#[derive(Debug, Clone, Default)]
pub struct Input<'a> {
    pub from: Option<&'a Address>,
    pub reply_to: &'a [Address],
    pub subject: &'a str,
    pub list_id: Option<&'a str>,
    /// `messages.auth_fail`: what the provider found failing.
    pub auth_fail: Option<&'a str>,
    /// The HTML as sent (for the links).
    pub html: Option<&'a str>,
}

/// What the store knows around a message, for [`analyze`].
#[derive(Debug, Clone, Default)]
pub struct Context {
    /// Registrable domains of the people the user wrote to.
    pub correspondents: Vec<String>,
    /// Addresses the user wrote to.
    pub written_to: HashSet<String>,
    /// No earlier mail from this sender and never written to.
    pub first_contact: bool,
    /// The provider found DMARC passing for the From's domain.
    pub dmarc_pass: bool,
}

/// The reasons a message looks like phishing (empty: none).
pub fn analyze(m: &Input, ctx: &Context) -> Vec<Reason> {
    let mut out = vec![];
    let Some(from) = m.from else { return out };
    let email = from.email.trim().to_lowercase();
    let Some(domain) = domain_of(&email) else { return out };
    let reg = registrable(&domain);
    let brand_domains: Vec<String> = BRANDS.iter().flat_map(|b| b.domains.iter().map(|d| d.to_string())).collect();

    // Punycode with look-alike letters, or such letters written directly.
    let shown = unicode_domain(&domain);
    let shown_reg = registrable(&shown);
    if (shown != domain || !domain.is_ascii()) && shown.split('.').any(mixed_script) {
        out.push(Reason::Punycode { domain: domain.clone(), shown: shown.clone() });
    }

    // A known domain imitated: brands first, then the user's correspondents.
    let mut known = brand_domains.clone();
    known.extend(ctx.correspondents.iter().cloned());
    if let Some(like) = lookalike_of(&reg, &known) {
        out.push(Reason::Lookalike { domain: shown_reg.clone(), like });
    }

    // Answers to another domain (not for mailing lists, nor for a sender that wrote before and
    // passed DMARC: shops answer through their help desk's domain).
    if m.list_id.is_none() && (ctx.first_contact || !ctx.dmarc_pass) {
        for r in m.reply_to {
            let r_email = r.email.trim().to_lowercase();
            let Some(rd) = domain_of(&r_email) else { continue };
            if registrable(&rd) != reg && !ctx.written_to.contains(&r_email) {
                out.push(Reason::ReplyTo { reply_to: r_email, from: domain.clone() });
                break;
            }
        }
    }

    // An address or a domain in the display name that is not the sender's.
    if let Some(name) = from.name.as_deref() {
        if let Some(shown) = address_in(name).filter(|a| *a != email) {
            out.push(Reason::NameAddress { shown, actual: email.clone() });
        } else if let Some(d) = domain_in(name).filter(|d| registrable(d) != reg) {
            out.push(Reason::NameAddress { shown: d, actual: email.clone() });
        }
    }

    // A first contact that claims to be a bank, a payment or a parcel service.
    if ctx.first_contact {
        let name = from.name.as_deref().unwrap_or_default().to_lowercase();
        let local = email.split('@').next().unwrap_or_default().to_string();
        let words = format!("{name} {local} {}", label(&reg));
        let subject = m.subject.to_lowercase();
        let own_brand = |b: &Brand| b.domains.iter().any(|d| same_or_sub(&domain, d));
        // The subject counts for banks, payment and parcel services (a newsletter about a
        // software company is no claim), and only for names that are no everyday word.
        let in_subject =
            |b: &Brand, t: &str| b.kind != ClaimKind::Company && t.len() >= 3 && t != "ups" && has_word(&subject, t);
        let claimed = BRANDS
            .iter()
            .find(|b| !own_brand(b) && b.tokens.iter().any(|t| has_word(&words, t) || in_subject(b, t)))
            .map(|b| (b.name.to_string(), b.kind))
            .or_else(|| {
                CLAIM_WORDS
                    .iter()
                    .find(|(w, _)| has_word(&name, w) && !(ctx.dmarc_pass && domain.contains(w)))
                    .map(|(_, k)| (from.name.clone().unwrap_or_default(), *k))
            });
        if let Some((claim, kind)) = claimed
            && !out.iter().any(|r| matches!(r, Reason::Lookalike { .. }))
        {
            out.push(Reason::FirstContact { claim, kind });
        }
    }

    // Links whose text shows another domain.
    if let Some(html) = m.html {
        let found = mismatched_links(html, &reg);
        if let Some((shown, target)) = found.first().cloned() {
            out.push(Reason::Link { shown, target, more: found.len() - 1 });
        }
    }

    // DMARC or SPF failed at the user's provider.
    if let Some(fail) = m.auth_fail.filter(|f| !f.trim().is_empty()) {
        out.push(Reason::AuthFailed { checks: fail.split_whitespace().map(str::to_string).collect() });
    }
    out
}

/// `word` in `text` as a word of its own (not inside another word: „dpd“ in „update“ is none).
fn has_word(text: &str, word: &str) -> bool {
    let mut from = 0;
    while let Some(i) = text[from..].find(word) {
        let at = from + i;
        let before = text[..at].chars().next_back();
        let after = text[at + word.len()..].chars().next();
        let edge = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric());
        if edge(before) && edge(after) {
            return true;
        }
        from = at + word.len();
    }
    false
}

/// An e-mail address written in a display name.
fn address_in(name: &str) -> Option<String> {
    name.split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '(' | ')' | '"' | '\'' | ',' | ';' | '[' | ']'))
        .find(|w| {
            let Some((l, d)) = w.split_once('@') else { return false };
            !l.is_empty() && d.contains('.') && d.rsplit('.').next().is_some_and(|tld| tld.len() >= 2)
        })
        .map(|w| w.trim_matches('.').to_lowercase())
}

/// A domain written in a display name or a link's text (`paypal.com`, `www.sparkasse.de/login`).
fn domain_in(text: &str) -> Option<String> {
    for w in text.split_whitespace() {
        let w = w.trim_matches(|c: char| matches!(c, '(' | ')' | '"' | '\'' | ',' | ';' | ':' | '<' | '>' | '[' | ']'));
        let w = w.strip_prefix("https://").or_else(|| w.strip_prefix("http://")).unwrap_or(w);
        let host = w.split(['/', '?', '#']).next().unwrap_or_default().trim_end_matches('.').to_lowercase();
        if host.contains('@') || !host.contains('.') {
            continue;
        }
        let labels: Vec<&str> = host.split('.').collect();
        let tld = labels.last().copied().unwrap_or_default();
        let ok = labels.iter().all(|l| !l.is_empty() && l.chars().all(|c| c.is_alphanumeric() || c == '-'))
            && tld.len() >= 2
            && tld.chars().all(|c| c.is_alphabetic());
        if ok {
            return Some(host);
        }
    }
    None
}

/// The links of an HTML message whose text shows a domain other than the one they open, as
/// (shown, opened) registrable domains. Links of mailing services and of the sender's own
/// domain in between are not counted; a redirect is followed to the address it hides.
pub fn mismatched_links(html: &str, sender: &str) -> Vec<(String, String)> {
    let mut out = vec![];
    for (href, text) in anchors(html) {
        let Some(shown) = domain_in(&text).map(|d| registrable(&unicode_domain(&d))) else { continue };
        let target_url = crate::tracking::clean_link(&href).unwrap_or(href.clone());
        let Ok(url) = url::Url::parse(target_url.trim()) else { continue };
        if !matches!(url.scheme(), "http" | "https") {
            continue;
        }
        let Some(host) = url.host_str().map(|h| h.to_lowercase()) else { continue };
        let target = registrable(&unicode_domain(&host));
        if target == shown
            || target == sender
            || CLICK_HOSTS.iter().any(|c| same_or_sub(&host, c))
            || out.iter().any(|(s, t)| *s == shown && *t == target)
        {
            continue;
        }
        out.push((shown, target));
    }
    out
}

/// The `<a href>` of an HTML text with the text inside each, as the HTML tokenizer reads them.
fn anchors(html: &str) -> Vec<(String, String)> {
    use html5ever::tendril::{SliceExt, StrTendril};
    use html5ever::tokenizer::{BufferQueue, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer};
    use std::cell::RefCell;

    #[derive(Default)]
    struct State {
        open: Option<(String, String)>,
        done: Vec<(String, String)>,
    }
    struct Links(RefCell<State>);
    impl TokenSink for Links {
        type Handle = ();
        fn process_token(&self, token: Token, _line: u64) -> TokenSinkResult<()> {
            let mut s = self.0.borrow_mut();
            match token {
                Token::TagToken(tag) if &*tag.name == "a" => {
                    if let Some(done) = s.open.take() {
                        s.done.push(done);
                    }
                    if tag.kind == TagKind::StartTag
                        && let Some(href) = tag.attrs.iter().find(|a| &*a.name.local == "href")
                    {
                        s.open = Some((crate::tracking::browser_url(&href.value), String::new()));
                    }
                }
                Token::CharacterTokens(t) => {
                    if let Some((_, text)) = s.open.as_mut()
                        && text.len() < 400
                    {
                        text.push_str(&t);
                    }
                }
                _ => {}
            }
            TokenSinkResult::Continue
        }
    }
    let input = BufferQueue::default();
    let t: StrTendril = html.to_tendril();
    input.push_back(t);
    let tok = Tokenizer::new(Links(RefCell::new(State::default())), Default::default());
    let _ = tok.feed(&input);
    tok.end();
    let mut s = tok.sink.0.take();
    if let Some(open) = s.open.take() {
        s.done.push(open);
    }
    s.done.into_iter().map(|(h, t)| (h, t.split_whitespace().collect::<Vec<_>>().join(" "))).collect()
}

/// What the provider's `Authentication-Results` (the newest; from one of the user's own
/// authentication servers, `own`) says failed: `dmarc` (for the From's domain) and `spf`
/// (`fail`, not `softfail`).
pub fn auth_failures(authentication: &str, from: &str, own: &[String]) -> Vec<&'static str> {
    let Some(from_domain) = crate::account::domain_of(from) else { return vec![] };
    let mut parts = authentication.split(';').map(str::trim);
    let first = parts.next().unwrap_or_default();
    let (id, results): (String, Vec<&str>) = if first.contains('=') {
        (String::new(), std::iter::once(first).chain(parts).collect())
    } else {
        let id = first.split_whitespace().next().unwrap_or_default().to_ascii_lowercase();
        (if id.is_empty() { id } else { crate::avatars::org_domain(&id) }, parts.collect())
    };
    if !own.contains(&id) {
        return vec![];
    }
    let mut out = vec![];
    for r in &results {
        let lower = r.to_ascii_lowercase();
        if lower.starts_with("dmarc=fail")
            && lower
                .split_whitespace()
                .any(|w| w.strip_prefix("header.from=").is_some_and(|d| d.trim_end_matches(';') == from_domain))
            && !out.contains(&"dmarc")
        {
            out.push("dmarc");
        }
        if lower.starts_with("spf=fail") && !out.contains(&"spf") {
            out.push("spf");
        }
    }
    out
}

/// The senders the user said are fine.
pub fn is_ok(store: &Store, email: &str) -> Result<bool> {
    Ok(store
        .conn()
        .query_row("SELECT 1 FROM phishing_ok WHERE email = ?1", [email.trim()], |_| Ok(()))
        .optional()?
        .is_some())
}

/// „Das ist in Ordnung“: no warning for this sender any more (`false`: again).
pub fn set_ok(store: &Store, email: &str, ok: bool) -> Result<()> {
    if ok {
        store.conn().execute(
            "INSERT INTO phishing_ok (email, at) VALUES (?1, ?2) ON CONFLICT (email) DO UPDATE SET at = excluded.at",
            params![email.trim().to_lowercase(), now()],
        )?;
    } else {
        store.conn().execute("DELETE FROM phishing_ok WHERE email = ?1", [email.trim()])?;
    }
    Ok(())
}

/// The registrable domains of the people the user wrote to (big mail providers left out: a
/// friend at Gmail makes no domain known).
pub fn correspondents(store: &Store) -> Result<(Vec<String>, HashSet<String>)> {
    let mut st = store.conn().prepare("SELECT email FROM contacts WHERE sent > 0 ORDER BY sent DESC LIMIT 2000")?;
    let emails: Vec<String> = st.query_map([], |r| r.get::<_, String>(0))?.filter_map(|r| r.ok()).collect();
    let mut domains: Vec<String> = emails
        .iter()
        .filter_map(|e| domain_of(e))
        .map(|d| registrable(&d))
        .filter(|d| !PROVIDERS.contains(&d.as_str()))
        .collect();
    domains.sort();
    domains.dedup();
    Ok((domains, emails.into_iter().map(|e| e.to_lowercase()).collect()))
}

/// The warning for a stored message (`html`: its HTML as sent, when the caller has it), or
/// `None`: no reason found, the sender is fine, the warning is off, or the message is the user's
/// own (Sent, Drafts, from one of the user's addresses).
pub fn check(store: &Store, id: i64, html: Option<&str>) -> Result<Option<Warning>> {
    if !crate::settings::load(store)?.phishing_warning {
        return Ok(None);
    }
    let d = store.message(id)?;
    let Some(from) = d.summary.from.clone() else { return Ok(None) };
    let row: Option<(Option<String>, bool, Option<String>, i64)> = store
        .conn()
        .query_row(
            "SELECT m.auth_fail, m.dmarc_pass, b.role, m.date FROM messages m JOIN mailboxes b ON b.id = m.mailbox_id
             WHERE m.id = ?1",
            [id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get::<_, Option<bool>>(1)?.unwrap_or(false),
                    r.get(2)?,
                    r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                ))
            },
        )
        .optional()?;
    let Some((auth_fail, dmarc_pass, role, date)) = row else { return Ok(None) };
    if d.draft || matches!(role.as_deref(), Some("sent" | "drafts")) || is_ok(store, &from.email)? {
        return Ok(None);
    }
    let own: Vec<String> = store.accounts()?.into_iter().map(|a| a.email.to_lowercase()).collect();
    if own.contains(&from.email.to_lowercase()) {
        return Ok(None);
    }
    let (correspondents, written_to) = correspondents(store)?;
    let earlier: bool = store.conn().query_row(
        "SELECT EXISTS (SELECT 1 FROM messages WHERE from_email = ?1 COLLATE NOCASE AND id <> ?2 AND date < ?3)",
        params![from.email, id, date],
        |r| r.get(0),
    )?;
    let ctx = Context {
        correspondents,
        first_contact: !earlier && !written_to.contains(&from.email.to_lowercase()),
        written_to,
        dmarc_pass,
    };
    let input = Input {
        from: Some(&from),
        reply_to: &d.reply_to,
        subject: &d.summary.subject,
        list_id: d.list_id.as_deref(),
        auth_fail: auth_fail.as_deref(),
        html: html.or(d.html.as_deref()),
    };
    let reasons = analyze(&input, &ctx);
    Ok((!reasons.is_empty()).then(|| Warning { sender: from.email.to_lowercase(), reasons }))
}

#[cfg(test)]
mod tests;
