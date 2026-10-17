//! The prompts, in the user's language. Each task's system prompt starts with a sentence of its
//! own ([`Task::opening`]); the fake provider of the tests tells the tasks apart by it.

use serde::{Deserialize, Serialize};

use super::ChatMessage;
use super::tone::Tone;
use crate::i18n::Lang;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Task {
    Summary,
    Reply,
    Rewrite,
    Translate,
    Ask,
    Priority,
    Extract,
}

/// Writing help in the compose sheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Style {
    Shorter,
    Friendlier,
    /// Formal, and „Sie“ in German.
    Formal,
    /// Grammar and spelling only.
    Grammar,
}

impl Task {
    pub const ALL: [Task; 7] =
        [Task::Summary, Task::Reply, Task::Rewrite, Task::Translate, Task::Ask, Task::Priority, Task::Extract];

    /// The first sentence of the task's system prompt.
    pub fn opening(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Task::Summary, Lang::De) => "Du fasst E-Mail-Unterhaltungen zusammen.",
            (Task::Summary, Lang::En) => "You summarize e-mail conversations.",
            (Task::Reply, Lang::De) => "Du entwirfst Antworten auf E-Mails.",
            (Task::Reply, Lang::En) => "You draft replies to e-mails.",
            (Task::Rewrite, Lang::De) => "Du überarbeitest E-Mail-Texte.",
            (Task::Rewrite, Lang::En) => "You revise e-mail texts.",
            (Task::Translate, Lang::De) => "Du übersetzt E-Mails.",
            (Task::Translate, Lang::En) => "You translate e-mails.",
            (Task::Ask, Lang::De) => "Du beantwortest Fragen zum Postfach des Nutzers.",
            (Task::Ask, Lang::En) => "You answer questions about the user's mailbox.",
            (Task::Priority, Lang::De) => "Du entscheidest, ob eine E-Mail wichtig ist.",
            (Task::Priority, Lang::En) => "You decide whether an e-mail is important.",
            (Task::Extract, Lang::De) => "Du findest Aufgaben und Termine in E-Mails.",
            (Task::Extract, Lang::En) => "You find tasks and dates in e-mails.",
        }
    }

    /// The task of a system prompt (the fake provider).
    pub fn of(system: &str) -> Option<Task> {
        Task::ALL.into_iter().find(|t| [Lang::De, Lang::En].iter().any(|l| system.starts_with(t.opening(*l))))
    }
}

/// A message of a conversation as the prompts show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailText {
    pub from: String,
    pub date: String,
    pub subject: String,
    pub text: String,
}

fn mail_block(m: &MailText, lang: Lang) -> String {
    let (from, date, subject) = match lang {
        Lang::De => ("Von", "Datum", "Betreff"),
        Lang::En => ("From", "Date", "Subject"),
    };
    format!("{from}: {}\n{date}: {}\n{subject}: {}\n\n{}", m.from, m.date, m.subject, m.text)
}

fn language_name(lang: Lang) -> &'static str {
    match lang {
        Lang::De => "Deutsch",
        Lang::En => "English",
    }
}

/// The name of a target language for translation (`de`, `en`, `fr`, …) in the user's language.
pub fn target_name(code: &str, lang: Lang) -> String {
    let names: &[(&str, &str, &str)] = &[
        ("de", "Deutsche", "German"),
        ("en", "Englische", "English"),
        ("fr", "Französische", "French"),
        ("es", "Spanische", "Spanish"),
        ("it", "Italienische", "Italian"),
        ("nl", "Niederländische", "Dutch"),
        ("pl", "Polnische", "Polish"),
        ("tr", "Türkische", "Turkish"),
        ("uk", "Ukrainische", "Ukrainian"),
    ];
    match names.iter().find(|(c, _, _)| c.eq_ignore_ascii_case(code.trim())) {
        Some((_, de, en)) => (if lang == Lang::De { *de } else { *en }).to_string(),
        None => code.trim().to_string(),
    }
}

/// A conversation's summary: what it is about, what was decided, what is open, who does what.
pub fn summary(thread: &[MailText], lang: Lang) -> Vec<ChatMessage> {
    let system = match lang {
        Lang::De => format!(
            "{} Schreibe auf Deutsch drei bis fünf kurze Stichpunkte mit „- “: worum es geht, was entschieden ist, was offen ist und wer was tun soll. Nenne Personen beim Namen, erfinde nichts und antworte nur mit den Stichpunkten.",
            Task::Summary.opening(lang)
        ),
        Lang::En => format!(
            "{} Write three to five short bullet points with “- ” in English: what it is about, what was decided, what is open and who does what. Name people, invent nothing and answer with the bullet points only.",
            Task::Summary.opening(lang)
        ),
    };
    let body = thread.iter().map(|m| mail_block(m, lang)).collect::<Vec<_>>().join("\n\n---\n\n");
    vec![ChatMessage::system(system), ChatMessage::user(body)]
}

/// The tone lines of a reply prompt, from what was learned on this device.
fn tone_lines(tone: &Tone, lang: Lang) -> String {
    let mut out = vec![];
    match lang {
        Lang::De => {
            if let Some(g) = &tone.greeting {
                out.push(format!("Anrede wie sonst: „{g} …“."));
            }
            if let Some(c) = &tone.closing {
                out.push(format!("Gruß wie sonst: „{c}“."));
            }
            match tone.informal {
                Some(true) => out.push("Duzt die Leute meist.".into()),
                Some(false) => out.push("Siezt die Leute meist.".into()),
                None => {}
            }
            if tone.words_per_sentence > 0 {
                out.push(format!("Sätze mit etwa {} Wörtern.", tone.words_per_sentence));
            }
            if !tone.samples.is_empty() {
                out.push(format!(
                    "Beispiele eigener Mails:\n{}",
                    tone.samples.iter().map(|s| format!("«{s}»")).collect::<Vec<_>>().join("\n")
                ));
            }
        }
        Lang::En => {
            if let Some(g) = &tone.greeting {
                out.push(format!("Usual greeting: “{g} …”."));
            }
            if let Some(c) = &tone.closing {
                out.push(format!("Usual closing: “{c}”."));
            }
            match tone.informal {
                Some(true) => out.push("Usually informal (German „du“).".into()),
                Some(false) => out.push("Usually formal (German „Sie“).".into()),
                None => {}
            }
            if tone.words_per_sentence > 0 {
                out.push(format!("Sentences of about {} words.", tone.words_per_sentence));
            }
            if !tone.samples.is_empty() {
                out.push(format!(
                    "Samples of own mail:\n{}",
                    tone.samples.iter().map(|s| format!("«{s}»")).collect::<Vec<_>>().join("\n")
                ));
            }
        }
    }
    out.join("\n")
}

/// A reply draft to the last message of `thread`, written as `me`, in the user's tone when one
/// was learned. `instructions`: what the user wants to say („zusagen, aber erst ab 10 Uhr“).
pub fn reply(thread: &[MailText], me: &str, tone: Option<&Tone>, instructions: &str, lang: Lang) -> Vec<ChatMessage> {
    let tone = tone.map(|t| tone_lines(t, lang)).filter(|t| !t.is_empty());
    let mut system = match lang {
        Lang::De => format!(
            "{} Schreibe den Text einer Antwort auf die letzte E-Mail im Namen von {me}, in der Sprache der Unterhaltung (meist Deutsch). Nur der Text: keine Betreffzeile, kein Zitat, keine Signatur, keine Erklärungen. Versprich nichts, was nicht in den Hinweisen steht; wo eine Entscheidung fehlt, schreibe [ ] als Lücke.",
            Task::Reply.opening(lang)
        ),
        Lang::En => format!(
            "{} Write the text of a reply to the last e-mail on behalf of {me}, in the language of the conversation (usually English). The text only: no subject line, no quote, no signature, no explanations. Promise nothing the notes do not say; where a decision is missing, leave a gap as [ ].",
            Task::Reply.opening(lang)
        ),
    };
    if let Some(t) = tone {
        system.push_str(match lang {
            Lang::De => "\n\nSo schreibt der Nutzer sonst:\n",
            Lang::En => "\n\nHow the user usually writes:\n",
        });
        system.push_str(&t);
    }
    let mut user = thread.iter().map(|m| mail_block(m, lang)).collect::<Vec<_>>().join("\n\n---\n\n");
    if !instructions.trim().is_empty() {
        user.push_str(match lang {
            Lang::De => "\n\n---\n\nHinweise für die Antwort: ",
            Lang::En => "\n\n---\n\nNotes for the reply: ",
        });
        user.push_str(instructions.trim());
    }
    vec![ChatMessage::system(system), ChatMessage::user(user)]
}

/// Writing help: the text as the style asks, in its own language.
pub fn rewrite(text: &str, style: Style, lang: Lang) -> Vec<ChatMessage> {
    let what = match (style, lang) {
        (Style::Shorter, Lang::De) => "Mache ihn deutlich kürzer und klarer.",
        (Style::Shorter, Lang::En) => "Make it clearly shorter and clearer.",
        (Style::Friendlier, Lang::De) => "Mache ihn freundlicher und wärmer, ohne übertrieben zu werden.",
        (Style::Friendlier, Lang::En) => "Make it friendlier and warmer without overdoing it.",
        (Style::Formal, Lang::De) => "Mache ihn förmlich und sieze die Empfänger („Sie“, „Ihnen“).",
        (Style::Formal, Lang::En) => "Make it formal (in a German text, address the readers with „Sie“).",
        (Style::Grammar, Lang::De) => {
            "Korrigiere nur Rechtschreibung, Grammatik und Zeichensetzung; ändere sonst nichts."
        }
        (Style::Grammar, Lang::En) => "Fix spelling, grammar and punctuation only; change nothing else.",
    };
    let system = match lang {
        Lang::De => format!(
            "{} {what} Behalte die Sprache des Textes, seinen Inhalt und alle Namen, Zahlen, Daten und Links. Antworte nur mit dem überarbeiteten Text, ohne Einleitung.",
            Task::Rewrite.opening(lang)
        ),
        Lang::En => format!(
            "{} {what} Keep the text's language, its content and every name, number, date and link. Answer with the revised text only, without an introduction.",
            Task::Rewrite.opening(lang)
        ),
    };
    vec![ChatMessage::system(system), ChatMessage::user(text.to_string())]
}

/// A message translated into `target` (a language code).
pub fn translate(m: &MailText, target: &str, lang: Lang) -> Vec<ChatMessage> {
    let name = target_name(target, lang);
    let system = match lang {
        Lang::De => format!(
            "{} Übersetze die E-Mail ins {name}. Behalte Absätze, Aufzählungen, Namen, Zahlen und Links. Antworte nur mit der Übersetzung des Textes.",
            Task::Translate.opening(lang)
        ),
        Lang::En => format!(
            "{} Translate the e-mail into {name}. Keep paragraphs, lists, names, numbers and links. Answer with the translation of the text only.",
            Task::Translate.opening(lang)
        ),
    };
    vec![ChatMessage::system(system), ChatMessage::user(format!("{}\n\n{}", m.subject, m.text))]
}

/// A question to the mailbox with the messages the local search found, numbered for the
/// answer's references.
pub fn ask(question: &str, sources: &[MailText], today: &str, lang: Lang) -> Vec<ChatMessage> {
    let system = match lang {
        Lang::De => format!(
            "{} Antworte auf {} und nur anhand der folgenden E-Mails. Jede E-Mail hat eine Nummer in eckigen Klammern; belege jede Aussage mit ihrer Nummer, zum Beispiel [2]. Wenn die E-Mails die Frage nicht beantworten, sag das ehrlich. Antworte kurz. Heute ist der {today}.",
            Task::Ask.opening(lang),
            language_name(lang)
        ),
        Lang::En => format!(
            "{} Answer in {} and only from the following e-mails. Each e-mail has a number in square brackets; support every statement with its number, for example [2]. If the e-mails do not answer the question, say so honestly. Be brief. Today is {today}.",
            Task::Ask.opening(lang),
            language_name(lang)
        ),
    };
    let mut user = String::new();
    user.push_str(match lang {
        Lang::De => "Frage: ",
        Lang::En => "Question: ",
    });
    user.push_str(question.trim());
    user.push_str(match lang {
        Lang::De => "\n\nE-Mails:",
        Lang::En => "\n\nE-mails:",
    });
    for (i, m) in sources.iter().enumerate() {
        user.push_str(&format!("\n\n[{}] {}", i + 1, mail_block(m, lang)));
    }
    vec![ChatMessage::system(system), ChatMessage::user(user)]
}

/// Whether a message is important, as JSON.
pub fn priority(m: &MailText, me: &str, lang: Lang) -> Vec<ChatMessage> {
    let system = match lang {
        Lang::De => format!(
            "{} Wichtig ist, was {me} persönlich betrifft und eine Antwort, Entscheidung oder Handlung braucht, eine Frist hat oder von Vorgesetzten, Kunden oder engen Kontakten kommt; Newsletter, Werbung und automatische Benachrichtigungen sind nicht wichtig. Antworte nur mit JSON: {{\"important\": true oder false, \"reason\": \"kurzer Grund auf Deutsch\"}}",
            Task::Priority.opening(lang)
        ),
        Lang::En => format!(
            "{} Important is what concerns {me} personally and needs an answer, decision or action, has a deadline, or comes from managers, customers or close contacts; newsletters, ads and automatic notifications are not important. Answer with JSON only: {{\"important\": true or false, \"reason\": \"short reason in English\"}}",
            Task::Priority.opening(lang)
        ),
    };
    vec![ChatMessage::system(system), ChatMessage::user(mail_block(m, lang))]
}

/// Tasks and dates of a message, as JSON.
pub fn extract(m: &MailText, today: &str, lang: Lang) -> Vec<ChatMessage> {
    let system = match lang {
        Lang::De => format!(
            "{} Heute ist der {today}. Finde, was der Empfänger tun soll (Aufgaben) und Termine mit Datum. Rechne Angaben wie „nächsten Dienstag“ in ein Datum um. Antworte nur mit JSON: {{\"items\": [{{\"kind\": \"task\" oder \"event\", \"title\": \"kurzer Titel auf Deutsch\", \"date\": \"JJJJ-MM-TT\" oder null, \"time\": \"HH:MM\" oder null, \"end\": \"HH:MM\" oder null, \"place\": \"Ort\" oder null}}]}}. Keine Einträge: {{\"items\": []}}.",
            Task::Extract.opening(lang)
        ),
        Lang::En => format!(
            "{} Today is {today}. Find what the recipient should do (tasks) and appointments with a date. Turn phrases like “next Tuesday” into a date. Answer with JSON only: {{\"items\": [{{\"kind\": \"task\" or \"event\", \"title\": \"short title in English\", \"date\": \"YYYY-MM-DD\" or null, \"time\": \"HH:MM\" or null, \"end\": \"HH:MM\" or null, \"place\": \"place\" or null}}]}}. Nothing found: {{\"items\": []}}.",
            Task::Extract.opening(lang)
        ),
    };
    vec![ChatMessage::system(system), ChatMessage::user(mail_block(m, lang))]
}

/// The JSON object in a model's answer (models like to wrap it in prose or a code fence).
pub fn json_in(text: &str) -> Option<serde_json::Value> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str(text.get(start..=end)?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mail() -> MailText {
        MailText {
            from: "Jonas Berger".into(),
            date: "16.10.2026".into(),
            subject: "Montag".into(),
            text: "Passt Montag?".into(),
        }
    }

    #[test]
    fn every_task_is_recognized_by_its_opening_in_both_languages() {
        for lang in [Lang::De, Lang::En] {
            let tone = Tone::default();
            let prompts = [
                summary(&[mail()], lang),
                reply(&[mail()], "Maria", Some(&tone), "", lang),
                rewrite("Hallo", Style::Formal, lang),
                translate(&mail(), "fr", lang),
                ask("Wann?", &[mail()], "2026-10-17", lang),
                priority(&mail(), "Maria", lang),
                extract(&mail(), "2026-10-17", lang),
            ];
            let tasks: Vec<Task> = prompts.iter().map(|p| Task::of(&p[0].content).unwrap()).collect();
            assert_eq!(tasks, Task::ALL);
        }
    }

    #[test]
    fn prompts_are_in_the_users_language() {
        let p = summary(&[mail()], Lang::De);
        assert!(p[0].content.contains("auf Deutsch"));
        assert!(p[1].content.starts_with("Von: Jonas Berger\nDatum: 16.10.2026\nBetreff: Montag"));
        assert!(summary(&[mail()], Lang::En)[0].content.contains("in English"));
        let f = rewrite("Kannst du mir das schicken?", Style::Formal, Lang::De);
        assert!(f[0].content.contains("sieze"));
        assert!(f[0].content.contains("Behalte die Sprache"));
        assert!(translate(&mail(), "fr", Lang::De)[0].content.contains("ins Französische"));
        assert!(translate(&mail(), "de", Lang::En)[0].content.contains("into German"));
        let a = ask("Wann kommt das Angebot?", &[mail(), mail()], "2026-10-17", Lang::De);
        assert!(a[1].content.contains("[2] Von: Jonas"));
    }

    #[test]
    fn reply_prompts_carry_the_tone_and_the_notes() {
        let tone = Tone {
            greeting: Some("Hallo".into()),
            closing: Some("Viele Grüße".into()),
            informal: Some(true),
            words_per_sentence: 11,
            samples: vec!["Hallo Lea, klingt gut!".into()],
            learned_from: 5,
            learned_at: 1,
        };
        let p = reply(&[mail()], "Maria Muster", Some(&tone), "zusagen", Lang::De);
        assert!(p[0].content.contains("Gruß wie sonst: „Viele Grüße“"));
        assert!(p[0].content.contains("Duzt"));
        assert!(p[0].content.contains("«Hallo Lea, klingt gut!»"));
        assert!(p[1].content.ends_with("Hinweise für die Antwort: zusagen"));
        let plain = reply(&[mail()], "Maria", None, "", Lang::En);
        assert!(!plain[0].content.contains("usually writes"));
    }

    #[test]
    fn json_in_prose() {
        let v = json_in("Sure! ```json\n{\"important\": true, \"reason\": \"Frist\"}\n```").unwrap();
        assert_eq!(v["important"], true);
        assert!(json_in("no json").is_none());
    }
}
