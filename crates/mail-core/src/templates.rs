//! Templates and snippets for the compose sheet. A template is a subject and a text; a snippet a
//! text behind a short trigger word, typed after `;;` in the message (`;;danke`). Both may hold
//! variables, filled when they are put in: `{name}`, `{first_name}` (the first recipient),
//! `{date}`, `{time}`, `{my_name}` (the sender name of the account), `{subject}` and
//! `{signature}` (the account's signature); the German names (`{vorname}`, `{datum}`, …) work
//! as well. A template of one account is offered only when writing from it.

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::store::{Store, now};
use crate::tr;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    #[default]
    Template,
    Snippet,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Template {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub kind: Kind,
    pub name: String,
    /// The snippet's trigger word (letters and digits), typed after `;;`.
    #[serde(default)]
    pub trigger: Option<String>,
    #[serde(default)]
    pub subject: String,
    pub body: String,
    /// `None`: every account.
    #[serde(default)]
    pub account_id: Option<i64>,
}

/// What the variables stand for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Vars {
    pub name: String,
    pub my_name: String,
    pub date: String,
    pub time: String,
    pub subject: String,
    pub signature: String,
}

/// A trigger word: lower case letters, digits, `-` and `_`.
pub fn clean_trigger(t: &str) -> String {
    t.trim()
        .trim_start_matches(';')
        .trim_start_matches('/')
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

pub fn list(store: &Store) -> Result<Vec<Template>> {
    let mut st = store.conn().prepare(
        "SELECT id, kind, name, trigger, subject, body, account_id FROM templates ORDER BY kind, lower(name), id",
    )?;
    let rows = st.query_map([], |r| {
        Ok(Template {
            id: r.get(0)?,
            kind: if r.get::<_, String>(1)? == "snippet" { Kind::Snippet } else { Kind::Template },
            name: r.get(2)?,
            trigger: r.get(3)?,
            subject: r.get(4)?,
            body: r.get(5)?,
            account_id: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Saves a template or snippet; returns its id.
pub fn save(store: &Store, t: &Template) -> Result<i64> {
    let name = t.name.trim();
    if name.is_empty() {
        return Err(Error::Invalid(tr!("Gib einen Namen ein.", "Enter a name.").into()));
    }
    if t.body.trim().is_empty() && t.subject.trim().is_empty() {
        return Err(Error::Invalid(tr!("Der Text fehlt.", "The text is missing.").into()));
    }
    let trigger = match t.kind {
        Kind::Snippet => {
            let tr = clean_trigger(t.trigger.as_deref().unwrap_or(name));
            if tr.is_empty() {
                return Err(Error::Invalid(
                    tr!("Das Kürzel braucht Buchstaben oder Ziffern.", "The trigger needs letters or digits.").into(),
                ));
            }
            let taken: bool = store.conn().query_row(
                "SELECT EXISTS (SELECT 1 FROM templates WHERE kind = 'snippet' AND trigger = ?1 AND id <> ?2)",
                params![tr, t.id],
                |r| r.get(0),
            )?;
            if taken {
                return Err(Error::Invalid(tr!("Dieses Kürzel gibt es schon.", "This trigger exists already.").into()));
            }
            Some(tr)
        }
        Kind::Template => None,
    };
    let kind = if t.kind == Kind::Snippet { "snippet" } else { "template" };
    if t.id > 0 {
        store.conn().execute(
            "UPDATE templates SET kind = ?2, name = ?3, trigger = ?4, subject = ?5, body = ?6, account_id = ?7, updated_at = ?8 WHERE id = ?1",
            params![t.id, kind, name, trigger, t.subject.trim(), t.body, t.account_id, now()],
        )?;
        return Ok(t.id);
    }
    store.conn().execute(
        "INSERT INTO templates (kind, name, trigger, subject, body, account_id, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![kind, name, trigger, t.subject.trim(), t.body, t.account_id, now()],
    )?;
    Ok(store.conn().last_insert_rowid())
}

pub fn delete(store: &Store, id: i64) -> Result<()> {
    store.conn().execute("DELETE FROM templates WHERE id = ?1", [id])?;
    Ok(())
}

/// Fills the variables of a text. Unknown `{…}` stay as they are.
pub fn render(text: &str, v: &Vars) -> String {
    let first = v.name.split_whitespace().next().unwrap_or_default().to_string();
    let pairs: [(&[&str], &str); 7] = [
        (&["name"], v.name.as_str()),
        (&["first_name", "vorname"], first.as_str()),
        (&["date", "datum"], v.date.as_str()),
        (&["time", "uhrzeit", "zeit"], v.time.as_str()),
        (&["my_name", "mein_name"], v.my_name.as_str()),
        (&["subject", "betreff"], v.subject.as_str()),
        (&["signature", "signatur"], v.signature.as_str()),
    ];
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end) if end <= 20 => {
                let key = after[..end].trim().to_lowercase();
                match pairs.iter().find(|(names, _)| names.contains(&key.as_str())) {
                    Some((_, value)) => out.push_str(value),
                    None => out.push_str(&rest[start..start + end + 2]),
                }
                rest = &after[end + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::store_with_inbox;

    #[test]
    fn variables() {
        let v = Vars {
            name: "Lea Weber".into(),
            my_name: "Maria Muster".into(),
            date: "17.10.2026".into(),
            time: "09:30".into(),
            subject: "Angebot".into(),
            signature: "Maria\nTel. 123".into(),
        };
        assert_eq!(
            render("Hallo {vorname}, danke für „{subject}“ vom {datum}.", &v),
            "Hallo Lea, danke für „Angebot“ vom 17.10.2026."
        );
        assert_eq!(render("{Name} – {my_name} {unbekannt} {x", &v), "Lea Weber – Maria Muster {unbekannt} {x");
        assert_eq!(render("{signature}", &v), "Maria\nTel. 123");
        assert_eq!(render("{ first_name }", &Vars::default()), "");
    }

    #[test]
    fn stored_with_unique_triggers() {
        let (s, account, _) = store_with_inbox();
        let snip = Template {
            id: 0,
            kind: Kind::Snippet,
            name: "Danke".into(),
            trigger: Some(";;Danke!".into()),
            subject: String::new(),
            body: "Vielen Dank, {vorname}!".into(),
            account_id: None,
        };
        let id = save(&s, &snip).unwrap();
        assert_eq!(list(&s).unwrap()[0].trigger.as_deref(), Some("danke"));
        assert!(save(&s, &Template { id: 0, ..snip.clone() }).unwrap_err().to_string().contains("exists"));
        save(&s, &Template { id, body: "Danke dir!".into(), ..snip.clone() }).unwrap();
        let t = Template {
            id: 0,
            kind: Kind::Template,
            name: "Angebot".into(),
            trigger: None,
            subject: "Ihr Angebot".into(),
            body: "Guten Tag {name},".into(),
            account_id: Some(account),
        };
        save(&s, &t).unwrap();
        let all = list(&s).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all.iter().find(|x| x.kind == Kind::Snippet).unwrap().body, "Danke dir!");
        assert!(save(&s, &Template { name: " ".into(), ..t.clone() }).is_err());
        delete(&s, id).unwrap();
        assert_eq!(list(&s).unwrap().len(), 1);
    }
}
