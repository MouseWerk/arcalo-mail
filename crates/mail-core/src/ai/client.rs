//! The OpenAI chat protocol (`/chat/completions` streamed as server-sent events, `/models`) as
//! Ollama, LM Studio, OpenAI, Anthropic's compatible endpoint, Mistral and Azure OpenAI speak
//! it. Every request goes through [`network::client_for_events`] (`Service::Ai`): no total
//! timeout, since a local model may need a while to load, but a read timeout.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::provider::{AiProvider, ProviderKind, parse_model_list};
use crate::error::{Error, Result};
use crate::network::{self, Service};
use crate::{tr, trf};

/// A model may need this long to load and start answering.
pub const FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(120);
/// A stream that sends nothing for this long is dead.
pub const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(c: impl Into<String>) -> Self {
        ChatMessage { role: "system".into(), content: c.into() }
    }
    pub fn user(c: impl Into<String>) -> Self {
        ChatMessage { role: "user".into(), content: c.into() }
    }
}

/// How an answer ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Answer {
    pub text: String,
    /// The user cancelled; `text` is what arrived until then.
    pub cancelled: bool,
}

/// Splits a byte stream into the `data:` payloads of server-sent events.
#[derive(Debug, Default)]
pub struct SseDecoder {
    buf: Vec<u8>,
    data: Vec<String>,
}

impl SseDecoder {
    /// Feeds bytes; returns the payloads of the events they complete.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(bytes);
        let mut events = vec![];
        while let Some(nl) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=nl).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                if !self.data.is_empty() {
                    events.push(self.data.join("\n"));
                    self.data.clear();
                }
            } else if let Some(rest) = line.strip_prefix("data:") {
                self.data.push(rest.strip_prefix(' ').unwrap_or(rest).to_owned());
            }
        }
        events
    }
}

/// The text of a stream chunk (`choices[0].delta.content`) or of a whole answer
/// (`choices[0].message.content`).
fn delta_text(v: &Value) -> Option<&str> {
    let c = &v["choices"][0];
    c["delta"]["content"].as_str().or_else(|| c["message"]["content"].as_str()).filter(|t| !t.is_empty())
}

/// What a provider's error means, in words the user can act on.
pub fn status_error(p: &AiProvider, status: u16, body: &str) -> Error {
    let name = p.display_name();
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v["error"]["message"]
                .as_str()
                .or_else(|| v["error"].as_str())
                .or_else(|| v["message"].as_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| body.split_whitespace().collect::<Vec<_>>().join(" "));
    let detail: String = detail.chars().take(200).collect();
    Error::Network(match status {
        401 | 403 => trf!(
            "„{name}“ hat den Schlüssel abgelehnt. Prüfe ihn in den Einstellungen unter KI.",
            "“{name}” refused the key. Check it in Settings under AI."
        ),
        404 => trf!(
            "„{name}“ kennt das Modell „{model}“ nicht. Wähle in den Einstellungen unter KI ein anderes.",
            "“{name}” does not know the model “{model}”. Choose another one in Settings under AI.",
            model = p.model
        ),
        429 => trf!(
            "„{name}“ ist gerade ausgelastet, oder dein Kontingent ist aufgebraucht. Versuche es später noch einmal.",
            "“{name}” is busy right now, or your quota is used up. Try again later."
        ),
        _ => {
            trf!("„{name}“ meldet einen Fehler ({status}): {detail}", "“{name}” reports an error ({status}): {detail}")
        }
    })
}

/// A connection that did not come about: for a provider on this computer, say that it does not
/// run.
fn unreachable(p: &AiProvider, e: &reqwest::Error) -> Error {
    let name = p.display_name();
    if p.local() && (e.is_connect() || e.is_timeout()) {
        let hint = match p.kind {
            ProviderKind::Ollama => tr!("Starte Ollama", "Start Ollama"),
            ProviderKind::LmStudio => {
                tr!("Starte in LM Studio den lokalen Server", "Start the local server in LM Studio")
            }
            _ => tr!("Starte den Server", "Start the server"),
        };
        return Error::Network(trf!(
            "„{name}“ antwortet nicht unter {url}. {hint} und versuche es noch einmal.",
            "“{name}” does not answer at {url}. {hint} and try again.",
            url = p.base_url
        ));
    }
    if e.is_timeout() {
        return Error::Network(trf!(
            "„{name}“ antwortet nicht (Zeitüberschreitung).",
            "“{name}” does not answer (timed out)."
        ));
    }
    Error::Network(trf!("„{name}“ ist nicht erreichbar: {e}", "“{name}” cannot be reached: {e}"))
}

/// A client of one provider with its key.
pub struct AiClient {
    provider: AiProvider,
    key: Option<String>,
    http: network::HttpClient,
}

impl AiClient {
    pub fn new(provider: AiProvider, key: Option<String>) -> Result<Self> {
        network::check_https_url(&provider.base_url)?;
        if provider.needs_key() && key.as_deref().is_none_or(|k| k.trim().is_empty()) {
            return Err(Error::Invalid(trf!(
                "Für „{}“ fehlt der Schlüssel. Trage ihn in den Einstellungen unter KI ein.",
                "The key for “{}” is missing. Enter it in Settings under AI.",
                provider.display_name()
            )));
        }
        Ok(AiClient { provider, key, http: network::client_for_events(Service::Ai)? })
    }

    pub fn provider(&self) -> &AiProvider {
        &self.provider
    }

    fn request(&self, method: reqwest::Method, url: &str) -> reqwest::RequestBuilder {
        let mut req = self.http.request(method, url);
        for (k, v) in self.provider.headers(self.key.as_deref()) {
            req = req.header(k, v);
        }
        req
    }

    /// Streams a chat completion: `on_delta` gets every piece of text as it arrives. When
    /// `cancel` becomes true the request ends and the text so far comes back, marked cancelled.
    pub async fn chat(
        &self,
        messages: &[ChatMessage],
        temperature: f32,
        cancel: &AtomicBool,
        mut on_delta: impl FnMut(&str),
    ) -> Result<Answer> {
        let model = self.provider.model.trim();
        if model.is_empty() {
            return Err(Error::Invalid(trf!(
                "Für „{}“ ist kein Modell gewählt. Wähle eines in den Einstellungen unter KI.",
                "No model is chosen for “{}”. Choose one in Settings under AI.",
                self.provider.display_name()
            )));
        }
        let url = self.provider.chat_url();
        network::check_https_url(&url)?;
        let body = json!({
            "model": model,
            "messages": messages,
            "stream": true,
            "temperature": temperature,
        });
        let cancelled = || cancel.load(Ordering::Relaxed);
        let wait_cancel = || async {
            loop {
                tokio::time::sleep(Duration::from_millis(80)).await;
                if cancelled() {
                    return;
                }
            }
        };
        let req = self
            .request(reqwest::Method::POST, &url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(serde_json::to_vec(&body)?);
        let send = tokio::time::timeout(FIRST_BYTE_TIMEOUT, req.send());
        let mut resp = tokio::select! {
            r = send => match r {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => return Err(unreachable(&self.provider, &e)),
                Err(_) => return Err(Error::Network(trf!(
                    "„{}“ hat nicht angefangen zu antworten (Zeitüberschreitung).",
                    "“{}” did not start to answer (timed out).",
                    self.provider.display_name()
                ))),
            },
            _ = wait_cancel() => return Ok(Answer { text: String::new(), cancelled: true }),
        };
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            return Err(status_error(&self.provider, status, &text));
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        let mut text = String::new();
        if content_type.contains("json") {
            // A server that ignores `stream`: the whole answer at once.
            let bytes = resp.bytes().await.map_err(|e| Error::Network(e.to_string()))?;
            let v: Value = serde_json::from_slice(&bytes)?;
            if let Some(t) = delta_text(&v) {
                on_delta(t);
                text.push_str(t);
            }
        } else {
            let mut decoder = SseDecoder::default();
            let waiter = wait_cancel();
            tokio::pin!(waiter);
            'stream: loop {
                let chunk = tokio::select! {
                    c = tokio::time::timeout(STREAM_IDLE_TIMEOUT, resp.chunk()) => match c {
                        Ok(Ok(Some(c))) => c,
                        Ok(Ok(None)) => break,
                        Ok(Err(e)) => return Err(unreachable(&self.provider, &e)),
                        Err(_) => return Err(Error::Network(trf!(
                            "„{}“ hat mitten in der Antwort aufgehört (Zeitüberschreitung).",
                            "“{}” stopped in the middle of the answer (timed out).",
                            self.provider.display_name()
                        ))),
                    },
                    _ = &mut waiter => return Ok(Answer { text, cancelled: true }),
                };
                for data in decoder.push(&chunk) {
                    if data.trim() == "[DONE]" {
                        break 'stream;
                    }
                    let Ok(v) = serde_json::from_str::<Value>(&data) else { continue };
                    if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
                        let body = json!({ "error": err }).to_string();
                        return Err(status_error(&self.provider, 500, &body));
                    }
                    if let Some(t) = delta_text(&v) {
                        on_delta(t);
                        text.push_str(t);
                    }
                }
            }
        }
        if text.trim().is_empty() {
            return Err(Error::Network(trf!(
                "„{}“ hat eine leere Antwort geschickt.",
                "“{}” sent an empty answer.",
                self.provider.display_name()
            )));
        }
        Ok(Answer { text, cancelled: false })
    }

    /// The models the provider offers (empty for Azure, whose deployments cannot be listed).
    pub async fn models(&self) -> Result<Vec<String>> {
        let Some(url) = self.provider.models_url() else { return Ok(vec![]) };
        network::check_https_url(&url)?;
        let resp = tokio::time::timeout(Duration::from_secs(20), self.request(reqwest::Method::GET, &url).send())
            .await
            .map_err(|_| {
                Error::Network(trf!(
                    "„{}“ antwortet nicht (Zeitüberschreitung).",
                    "“{}” does not answer (timed out).",
                    self.provider.display_name()
                ))
            })?
            .map_err(|e| unreachable(&self.provider, &e))?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            return Err(status_error(&self.provider, status, &text));
        }
        let bytes = resp.bytes().await.map_err(|e| Error::Network(e.to_string()))?;
        let v: Value = serde_json::from_slice(&bytes)?;
        let mut list = parse_model_list(&v);
        list.sort();
        list.dedup();
        Ok(list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_decoder_handles_split_chunks_and_comments() {
        let mut d = SseDecoder::default();
        assert!(d.push(b": keep-alive\n\ndata: {\"a\"").is_empty());
        assert_eq!(d.push(b":1}\r\n\r\ndata: [DONE]\n"), vec!["{\"a\":1}".to_string()]);
        assert_eq!(d.push(b"\n"), vec!["[DONE]".to_string()]);
    }

    #[test]
    fn errors_say_what_to_do() {
        let p = AiProvider { name: "OpenAI".into(), model: "gpt-x".into(), ..AiProvider::new(ProviderKind::Openai) };
        assert!(status_error(&p, 401, "{}").to_string().contains("refused the key"));
        assert!(status_error(&p, 404, "").to_string().contains("“gpt-x”"));
        let e = status_error(&p, 500, r#"{"error":{"message":"overloaded"}}"#).to_string();
        assert!(e.contains("(500): overloaded"), "{e}");
        let missing = AiClient::new(p.clone(), None).err().unwrap().to_string();
        assert!(missing.contains("key for “OpenAI” is missing"), "{missing}");
        assert!(AiClient::new(AiProvider::new(ProviderKind::Ollama), None).is_ok(), "a local model needs no key");
    }
}
