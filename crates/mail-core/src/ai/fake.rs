//! An OpenAI-compatible AI provider in memory, for the tests of the core and the test server
//! (`crates/mail-testserver`, the e2e tests and the screenshots): `/models`, `/api/tags` and a
//! streamed `/chat/completions` that answers each task ([`Task::of`]) with a text made from the
//! prompt, or with an answer the test set. It notes every request, so a test can check what
//! was sent and that nothing was sent where nothing may go.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Value, json};

use super::prompts::Task;

/// A request the fake took.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FakeRequest {
    pub path: String,
    pub task: Option<Task>,
    pub model: String,
    /// The key that came with it (`Authorization: Bearer`, `api-key`, `x-api-key`).
    pub key: Option<String>,
    pub system: String,
    pub user: String,
}

/// What to send back: a status, a content type and the body in pieces (sent one after the other
/// with a pause, so a stream can be watched and cancelled).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeReply {
    pub status: u16,
    pub content_type: &'static str,
    pub chunks: Vec<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct FakeAi {
    pub models: Vec<String>,
    /// The key requests must carry (`None`: any or none).
    pub key: Option<String>,
    /// Answers set by the test per task (`summary`, `reply`, …), instead of the made-up ones.
    pub answers: BTreeMap<String, String>,
    /// The next chat request fails with this status.
    pub fail: Option<u16>,
    pub requests: Vec<FakeRequest>,
    /// Characters per streamed piece.
    pub piece: usize,
}

impl Default for FakeAi {
    fn default() -> Self {
        FakeAi {
            models: vec!["llama3.2".into(), "qwen3:8b".into()],
            key: None,
            answers: BTreeMap::new(),
            fail: None,
            requests: vec![],
            piece: 6,
        }
    }
}

fn task_name(t: Task) -> &'static str {
    match t {
        Task::Summary => "summary",
        Task::Reply => "reply",
        Task::Rewrite => "rewrite",
        Task::Translate => "translate",
        Task::Ask => "ask",
        Task::Priority => "priority",
        Task::Extract => "extract",
    }
}

fn json_reply(status: u16, v: Value) -> FakeReply {
    FakeReply { status, content_type: "application/json", chunks: vec![v.to_string().into_bytes()] }
}

/// The value of a `Label: value` line of a prompt.
fn field<'a>(text: &'a str, labels: &[&str]) -> Option<&'a str> {
    text.lines().find_map(|l| labels.iter().find_map(|lab| l.strip_prefix(lab)).map(str::trim))
}

impl FakeAi {
    /// Answers one request; `headers` lower-cased.
    pub fn handle(&mut self, method: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> FakeReply {
        let key = headers.iter().find_map(|(k, v)| match k.as_str() {
            "authorization" => Some(v.trim_start_matches("Bearer ").to_string()),
            "api-key" | "x-api-key" => Some(v.clone()),
            _ => None,
        });
        if let Some(want) = &self.key
            && key.as_deref() != Some(want.as_str())
        {
            return json_reply(401, json!({"error": {"message": "Incorrect API key provided"}}));
        }
        let p = path.split('?').next().unwrap_or_default();
        if method == "GET" && (p.ends_with("/models") || p.ends_with("/api/tags")) {
            let data: Vec<Value> = self.models.iter().map(|m| json!({"id": m, "name": m})).collect();
            return json_reply(200, json!({"data": data, "models": data}));
        }
        if method != "POST" || !p.ends_with("/chat/completions") {
            return json_reply(404, json!({"error": {"message": "not found"}}));
        }
        let v: Value = serde_json::from_slice(body).unwrap_or_default();
        let msgs = v["messages"].as_array().cloned().unwrap_or_default();
        let content = |role: &str| {
            msgs.iter()
                .filter(|m| m["role"] == role)
                .filter_map(|m| m["content"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        };
        let (system, user) = (content("system"), content("user"));
        let task = Task::of(&system);
        let model = v["model"].as_str().unwrap_or_default().to_string();
        self.requests.push(FakeRequest {
            path: p.to_string(),
            task,
            model: model.clone(),
            key,
            system: system.clone(),
            user: user.clone(),
        });
        if let Some(status) = self.fail.take() {
            return json_reply(status, json!({"error": {"message": "the fake provider failed on purpose"}}));
        }
        if !self.models.contains(&model) {
            return json_reply(404, json!({"error": {"message": format!("model '{model}' not found")}}));
        }
        let answer =
            task.and_then(|t| self.answers.get(task_name(t)).cloned()).unwrap_or_else(|| made_up(task, &system, &user));
        let mut chunks: Vec<Vec<u8>> = vec![];
        let chars: Vec<char> = answer.chars().collect();
        for piece in chars.chunks(self.piece.max(1)) {
            let text: String = piece.iter().collect();
            let event = json!({"choices": [{"index": 0, "delta": {"content": text}}]});
            chunks.push(format!("data: {event}\n\n").into_bytes());
        }
        let end = json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]});
        chunks.push(format!("data: {end}\n\ndata: [DONE]\n\n").into_bytes());
        FakeReply { status: 200, content_type: "text/event-stream", chunks }
    }
}

/// A plausible answer from the prompt alone.
fn made_up(task: Option<Task>, system: &str, user: &str) -> String {
    let de = system.contains("Deutsch") || system.starts_with("Du ");
    let from = field(user, &["Von: ", "From: "]).unwrap_or("?").to_string();
    let subject = field(user, &["Betreff: ", "Subject: "]).unwrap_or_default().to_string();
    let count = user.matches("\n---\n").count() + 1;
    match task {
        Some(Task::Summary) if de => format!(
            "- {from} schreibt über „{subject}“.\n- Die Unterhaltung hat {count} Nachrichten.\n- Offen: deine Antwort."
        ),
        Some(Task::Summary) => {
            format!(
                "- {from} writes about “{subject}”.\n- The conversation has {count} messages.\n- Open: your answer."
            )
        }
        Some(Task::Reply) => {
            let last_from = user.rsplit("\n---\n").next().and_then(|b| field(b, &["Von: ", "From: "])).unwrap_or(&from);
            let first = last_from.split_whitespace().next().unwrap_or(last_from);
            let greeting = field(system, &["Anrede wie sonst: „"]).map(|g| g.trim_end_matches(" …“.").to_string());
            let closing = field(system, &["Gruß wie sonst: „"]).map(|c| c.trim_end_matches("“.").to_string());
            if de {
                format!(
                    "{} {first},\n\ndanke für deine Nachricht. Das passt mir gut, ich melde mich bis morgen mit den Details.\n\n{}",
                    greeting.unwrap_or_else(|| "Hallo".into()),
                    closing.unwrap_or_else(|| "Viele Grüße".into())
                )
            } else {
                format!(
                    "Hi {first},\n\nthanks for your message. That works for me; I will send the details by tomorrow.\n\nBest regards"
                )
            }
        }
        Some(Task::Rewrite) => {
            let text = user.trim();
            if system.contains("sieze") || system.contains("formal") {
                let mut out = String::new();
                for (i, w) in text.split(' ').enumerate() {
                    if i > 0 {
                        out.push(' ');
                    }
                    let core = w.trim_matches(|c: char| !c.is_alphanumeric());
                    let rep = match core {
                        "du" | "Du" => "Sie",
                        "dir" | "Dir" => "Ihnen",
                        "dich" | "Dich" => "Sie",
                        "dein" | "Dein" => "Ihr",
                        "deine" | "Deine" => "Ihre",
                        "Hallo" | "Hi" => "Guten Tag",
                        "kannst" => "können",
                        "Kannst" => "Können",
                        "hast" => "haben",
                        "Hast" => "Haben",
                        "bist" => "sind",
                        "Bist" => "Sind",
                        "schickst" => "schicken",
                        _ => core,
                    };
                    out.push_str(&w.replace(core, rep));
                }
                out
            } else if system.contains("kürzer") || system.contains("shorter") {
                let first = text.split_inclusive(['.', '!', '?']).next().unwrap_or(text);
                first.trim().to_string()
            } else if system.contains("freundlicher") || system.contains("friendlier") {
                if de {
                    format!("{text} Vielen Dank dir und einen schönen Tag!")
                } else {
                    format!("{text} Thanks so much, have a lovely day!")
                }
            } else {
                text.replace(" ,", ",").replace("  ", " ")
            }
        }
        Some(Task::Translate) => {
            let target = system
                .split(" ins ")
                .nth(1)
                .or_else(|| system.split(" into ").nth(1))
                .and_then(|t| t.split('.').next())
                .unwrap_or("?");
            format!("[{target}] {}", user.trim())
        }
        Some(Task::Ask) => {
            let first =
                user.split("\n\n[1] ").nth(1).and_then(|b| field(b, &["Von: ", "From: "]).or_else(|| b.lines().next()));
            match first {
                Some(f) if de => format!("Laut der Mail von {} geht es darum [1].", f.trim_start_matches("Von: ")),
                Some(f) => format!("According to the mail from {} [1].", f.trim_start_matches("From: ")),
                None if de => "Dazu steht nichts in den Mails.".into(),
                None => "The e-mails do not say.".into(),
            }
        }
        Some(Task::Priority) => {
            let important =
                user.contains('?') || user.to_lowercase().contains("bitte") || user.to_lowercase().contains("please");
            let reason = if de { "Braucht eine Antwort von dir" } else { "Needs an answer from you" };
            json!({"important": important, "reason": reason}).to_string()
        }
        Some(Task::Extract) => {
            let today = system
                .split(", ")
                .find_map(|s| s.get(..10).filter(|d| d.len() == 10 && d.as_bytes()[4] == b'-'))
                .unwrap_or("2026-10-17");
            let (y, m, d) = (
                today[..4].parse::<i64>().unwrap_or(2026),
                today[5..7].parse::<u32>().unwrap_or(10),
                today[8..10].parse::<u32>().unwrap_or(17),
            );
            let (y2, m2, d2, _) = super::civil((super::days_from_civil(y, m, d) + 3) * 86400);
            let title = if subject.is_empty() { "?".to_string() } else { subject.clone() };
            let task_title = if de { format!("Antworten: {title}") } else { format!("Reply: {title}") };
            json!({"items": [
                {"kind": "event", "title": title, "date": format!("{y2:04}-{m2:02}-{d2:02}"), "time": "14:00", "end": "15:00", "place": null},
                {"kind": "task", "title": task_title, "date": format!("{y2:04}-{m2:02}-{d2:02}"), "time": null}
            ]})
            .to_string()
        }
        None => {
            if de {
                "Hallo!".into()
            } else {
                "Hello!".into()
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod server {
    //! The fake on a loopback port, piece by piece with a pause (for the client's tests).

    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    use super::FakeAi;

    pub(crate) async fn serve(fake: Arc<Mutex<FakeAi>>, pause: Duration) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((s, _)) = listener.accept().await else { return };
                let fake = fake.clone();
                tokio::spawn(async move {
                    let mut r = BufReader::new(s);
                    let mut first = String::new();
                    if r.read_line(&mut first).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let mut parts = first.split_whitespace();
                    let method = parts.next().unwrap_or_default().to_string();
                    let target = parts.next().unwrap_or("/").to_string();
                    let mut headers = vec![];
                    let mut length = 0usize;
                    loop {
                        let mut h = String::new();
                        if r.read_line(&mut h).await.unwrap_or(0) == 0 || h.trim().is_empty() {
                            break;
                        }
                        if let Some((k, v)) = h.split_once(':') {
                            if k.trim().eq_ignore_ascii_case("content-length") {
                                length = v.trim().parse().unwrap_or(0);
                            }
                            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
                        }
                    }
                    let mut body = vec![0u8; length];
                    if r.read_exact(&mut body).await.is_err() {
                        return;
                    }
                    let reply = fake.lock().unwrap().handle(&method, &target, &headers, &body);
                    let head = format!(
                        "HTTP/1.1 {} X\r\nContent-Type: {}\r\nConnection: close\r\n\r\n",
                        reply.status, reply.content_type
                    );
                    let s = r.get_mut();
                    if s.write_all(head.as_bytes()).await.is_err() {
                        return;
                    }
                    for c in reply.chunks {
                        if s.write_all(&c).await.is_err() || s.flush().await.is_err() {
                            return;
                        }
                        tokio::time::sleep(pause).await;
                    }
                });
            }
        });
        port
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::*;
    use crate::ai::{AiClient, AiProvider, ChatMessage, ProviderKind};

    fn provider(port: u16, kind: ProviderKind) -> AiProvider {
        let base = match kind {
            ProviderKind::Ollama => format!("http://127.0.0.1:{port}"),
            _ => format!("http://127.0.0.1:{port}/v1"),
        };
        AiProvider {
            id: "fake".into(),
            name: "Fake".into(),
            kind,
            base_url: base,
            model: "llama3.2".into(),
            api_version: String::new(),
        }
    }

    #[tokio::test]
    async fn streams_answers_lists_models_and_reports_errors() {
        let fake = Arc::new(Mutex::new(FakeAi::default()));
        let port = server::serve(fake.clone(), Duration::from_millis(2)).await;
        let client = AiClient::new(provider(port, ProviderKind::Ollama), None).unwrap();
        assert_eq!(client.models().await.unwrap(), ["llama3.2", "qwen3:8b"]);
        let msgs = crate::ai::prompts::summary(
            &[crate::ai::prompts::MailText {
                from: "Jonas".into(),
                date: "x".into(),
                subject: "Montag".into(),
                text: "Passt Montag?".into(),
            }],
            crate::i18n::Lang::De,
        );
        let cancel = AtomicBool::new(false);
        let mut pieces = vec![];
        let a = client.chat(&msgs, 0.2, &cancel, |d| pieces.push(d.to_string())).await.unwrap();
        assert!(pieces.len() > 3, "streamed in pieces: {pieces:?}");
        assert_eq!(a.text, pieces.concat());
        assert!(a.text.starts_with("- Jonas schreibt über „Montag“."), "{}", a.text);
        assert!(!a.cancelled);
        {
            let f = fake.lock().unwrap();
            assert_eq!(f.requests[0].task, Some(Task::Summary));
            assert_eq!(f.requests[0].path, "/v1/chat/completions");
            assert_eq!(f.requests[0].key, None, "a local provider gets no key");
        }
        fake.lock().unwrap().fail = Some(500);
        let e = client.chat(&msgs, 0.2, &cancel, |_| {}).await.unwrap_err().to_string();
        assert!(e.contains("(500): the fake provider failed on purpose"), "{e}");
        let unknown =
            AiClient::new(AiProvider { model: "gpt-9".into(), ..provider(port, ProviderKind::Ollama) }, None).unwrap();
        assert!(unknown.chat(&msgs, 0.2, &cancel, |_| {}).await.unwrap_err().to_string().contains("“gpt-9”"));
    }

    #[tokio::test]
    async fn a_cancelled_stream_ends_with_what_arrived() {
        let fake = Arc::new(Mutex::new(FakeAi { piece: 1, ..Default::default() }));
        fake.lock().unwrap().answers.insert("rewrite".into(), "x".repeat(400));
        let port = server::serve(fake.clone(), Duration::from_millis(20)).await;
        let client = AiClient::new(provider(port, ProviderKind::LmStudio), None).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let msgs = crate::ai::prompts::rewrite("Hallo", crate::ai::prompts::Style::Shorter, crate::i18n::Lang::En);
        let mut n = 0;
        let a = client
            .chat(&msgs, 0.2, &cancel, |_| {
                n += 1;
                if n == 5 {
                    stop.store(true, Ordering::Relaxed);
                }
            })
            .await
            .unwrap();
        assert!(a.cancelled);
        assert!(a.text.len() >= 5 && a.text.len() < 400, "{}", a.text.len());
    }

    #[tokio::test]
    async fn cloud_providers_send_their_key() {
        let fake = Arc::new(Mutex::new(FakeAi { key: Some("sk-test".into()), ..Default::default() }));
        let port = server::serve(fake.clone(), Duration::from_millis(1)).await;
        // A cloud provider's kind with a loopback address: the key is still sent when given.
        let p = provider(port, ProviderKind::Openai);
        let wrong = AiClient::new(p.clone(), Some("sk-wrong".into())).unwrap();
        assert!(wrong.models().await.unwrap_err().to_string().contains("refused the key"));
        let ok = AiClient::new(p, Some("sk-test".into())).unwrap();
        let msgs = vec![
            ChatMessage::system("You translate e-mails. Translate the e-mail into German."),
            ChatMessage::user("Hello"),
        ];
        let a = ok.chat(&msgs, 0.1, &AtomicBool::new(false), |_| {}).await.unwrap();
        assert_eq!(a.text, "[German] Hello");
        assert_eq!(fake.lock().unwrap().requests[0].key.as_deref(), Some("sk-test"));
        let anthropic = AiClient::new(provider(port, ProviderKind::Anthropic), Some("sk-test".into())).unwrap();
        assert!(anthropic.models().await.is_ok());
    }
}
