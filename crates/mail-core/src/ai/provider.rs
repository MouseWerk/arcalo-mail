//! AI providers: where the assistant's requests go, as in Arcalo (`ai/provider.rs` there). A
//! local model (Ollama, LM Studio, any OpenAI-compatible server on this computer) or a cloud
//! provider the user adds (OpenAI, Anthropic, Mistral, Azure OpenAI, any OpenAI-compatible
//! API). All of them speak the OpenAI chat protocol; they differ in the paths, the key header
//! and the model list. Keys are not part of the settings: the credential store keeps one per
//! provider ([`key_name`]).

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::network;
use crate::{tr, trf};

/// Default address of a local Ollama.
pub const OLLAMA_URL: &str = "http://localhost:11434";
/// Default address of LM Studio's local server.
pub const LM_STUDIO_URL: &str = "http://localhost:1234/v1";
/// `api-version` of Azure OpenAI when none is set.
pub const AZURE_API_VERSION: &str = "2024-10-21";
/// The `anthropic-version` header of Anthropic's API.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Ollama: `<root>/v1/…` (OpenAI-compatible), its own `/api/tags`, no key.
    #[default]
    Ollama,
    /// LM Studio's local server (`http://localhost:1234/v1`), no key.
    LmStudio,
    /// OpenAI (`https://api.openai.com/v1`).
    Openai,
    /// Anthropic through its OpenAI-compatible endpoint (`https://api.anthropic.com/v1`).
    Anthropic,
    /// Mistral (`https://api.mistral.ai/v1`).
    Mistral,
    /// Azure OpenAI: `<endpoint>/openai/deployments/<deployment>/…?api-version=…` with an
    /// `api-key` header; the model is the deployment's name.
    Azure,
    /// Any OpenAI-compatible API; the address includes the version (`…/v1`). Unknown kinds (from
    /// a newer version) load as this.
    #[serde(other)]
    Compatible,
}

impl ProviderKind {
    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::Ollama => "Ollama",
            ProviderKind::LmStudio => "LM Studio",
            ProviderKind::Openai => "OpenAI",
            ProviderKind::Anthropic => "Anthropic",
            ProviderKind::Mistral => "Mistral",
            ProviderKind::Azure => "Azure OpenAI",
            ProviderKind::Compatible => tr!("OpenAI-kompatibel", "OpenAI-compatible"),
        }
    }

    /// The address a new provider of this kind starts with.
    pub fn default_url(self) -> &'static str {
        match self {
            ProviderKind::Ollama => OLLAMA_URL,
            ProviderKind::LmStudio => LM_STUDIO_URL,
            ProviderKind::Openai => "https://api.openai.com/v1",
            ProviderKind::Anthropic => "https://api.anthropic.com/v1",
            ProviderKind::Mistral => "https://api.mistral.ai/v1",
            ProviderKind::Azure | ProviderKind::Compatible => "",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiProvider {
    /// Stable id (`[a-z0-9-]`): named by the account choices and the credential store.
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    /// The model (Azure: the deployment) the assistant uses.
    pub model: String,
    /// Azure OpenAI: the `api-version` query parameter.
    pub api_version: String,
}

impl Default for AiProvider {
    fn default() -> Self {
        AiProvider {
            id: String::new(),
            name: String::new(),
            kind: ProviderKind::Ollama,
            base_url: String::new(),
            model: String::new(),
            api_version: String::new(),
        }
    }
}

/// The credential store name of a provider's key.
pub fn key_name(provider_id: &str) -> String {
    format!("ai-{provider_id}-key")
}

/// Whether `url` points at this computer (`localhost`, `127.x`, `::1`).
pub fn is_loopback(url: &str) -> bool {
    let Ok(u) = url::Url::parse(url.trim()) else { return false };
    u.host_str().is_some_and(network::is_loopback)
}

impl AiProvider {
    /// A provider of `kind` at its usual address.
    pub fn new(kind: ProviderKind) -> Self {
        AiProvider { name: kind.label().into(), kind, base_url: kind.default_url().into(), ..Default::default() }
    }

    /// Runs on this computer: mail it sees does not leave the device.
    pub fn local(&self) -> bool {
        is_loopback(&self.base_url)
    }

    /// The host requests go to, for the privacy line („Geht an api.openai.com“).
    pub fn host(&self) -> String {
        url::Url::parse(self.base_url.trim())
            .ok()
            .and_then(|u| u.host_str().map(|h| h.trim_matches(['[', ']']).to_string()))
            .unwrap_or_default()
    }

    /// The address without what the client adds itself (`/v1` for Ollama, `/openai` for Azure)
    /// and without a trailing slash.
    pub fn root(&self) -> String {
        let mut u = self.base_url.trim().trim_end_matches('/').to_owned();
        let strip: &[&str] = match self.kind {
            ProviderKind::Ollama => &["/v1", "/api"],
            ProviderKind::Azure => &["/openai"],
            _ => &[],
        };
        for s in strip {
            if u.len() > s.len() && u.get(u.len() - s.len()..).is_some_and(|t| t.eq_ignore_ascii_case(s)) {
                u.truncate(u.len() - s.len());
                u = u.trim_end_matches('/').to_owned();
            }
        }
        u
    }

    fn api_version(&self) -> &str {
        match self.api_version.trim() {
            "" => AZURE_API_VERSION,
            v => v,
        }
    }

    pub fn chat_url(&self) -> String {
        let root = self.root();
        match self.kind {
            ProviderKind::Ollama => format!("{root}/v1/chat/completions"),
            ProviderKind::Azure => format!(
                "{root}/openai/deployments/{}/chat/completions?api-version={}",
                path_segment(self.model.trim()),
                self.api_version()
            ),
            _ => format!("{root}/chat/completions"),
        }
    }

    /// The model list; `None` for Azure (deployments cannot be listed with a key).
    pub fn models_url(&self) -> Option<String> {
        let root = self.root();
        match self.kind {
            ProviderKind::Ollama => Some(format!("{root}/api/tags")),
            ProviderKind::Azure => None,
            _ => Some(format!("{root}/models")),
        }
    }

    /// Whether requests need a key: never for Ollama and LM Studio, for other APIs unless they
    /// run on this computer.
    pub fn needs_key(&self) -> bool {
        match self.kind {
            ProviderKind::Ollama | ProviderKind::LmStudio => false,
            _ => !self.local(),
        }
    }

    /// The key's headers: `api-key` for Azure, `x-api-key` and the version for Anthropic (its
    /// compatible endpoint also takes the bearer token), a bearer token otherwise.
    pub fn headers(&self, key: Option<&str>) -> Vec<(&'static str, String)> {
        let Some(k) = key.map(str::trim).filter(|k| !k.is_empty()) else { return vec![] };
        match self.kind {
            ProviderKind::Azure => vec![("api-key", k.to_string())],
            ProviderKind::Anthropic => vec![
                ("authorization", format!("Bearer {k}")),
                ("x-api-key", k.to_string()),
                ("anthropic-version", ANTHROPIC_VERSION.to_string()),
            ],
            _ => vec![("authorization", format!("Bearer {k}"))],
        }
    }

    /// Display name: the name, or the kind's label.
    pub fn display_name(&self) -> &str {
        match self.name.trim() {
            "" => self.kind.label(),
            n => n,
        }
    }
}

/// `s` percent-encoded as one URL path segment.
fn path_segment(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// `[a-z0-9-]` from a name: `Mistral AI` → `mistral-ai`.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.trim().to_lowercase().chars() {
        let plain = match c {
            'ä' => "a",
            'ö' => "o",
            'ü' => "u",
            'ß' => "ss",
            _ => "",
        };
        if !plain.is_empty() {
            out.push_str(plain);
        } else if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').chars().take(40).collect()
}

/// Trims the providers, checks their addresses (`https://`, or `http://` to this computer only,
/// as every other connection of the app), and gives each a unique id and a name.
pub fn normalize(list: Vec<AiProvider>) -> Result<Vec<AiProvider>> {
    let mut seen: Vec<String> = vec![];
    let mut out = vec![];
    for mut p in list {
        p.name = p.name.trim().to_owned();
        if p.name.is_empty() {
            p.name = p.kind.label().to_owned();
        }
        p.base_url = p.base_url.trim().trim_end_matches('/').to_owned();
        if p.base_url.is_empty() {
            p.base_url = p.kind.default_url().to_owned();
        }
        p.model = p.model.trim().to_owned();
        p.api_version = p.api_version.trim().to_owned();
        if url::Url::parse(&p.base_url).is_err() || network::check_https_url(&p.base_url).is_err() {
            return Err(Error::Invalid(trf!(
                "Die Adresse von „{}“ muss mit https:// beginnen (http:// nur auf diesem Gerät, z. B. http://localhost:11434).",
                "The address of “{}” must start with https:// (http:// only on this device, e.g. http://localhost:11434).",
                p.name
            )));
        }
        let base = match slug(&p.id) {
            s if !s.is_empty() => s,
            _ => match slug(&p.name) {
                s if !s.is_empty() => s,
                _ => "provider".into(),
            },
        };
        let mut id = base.clone();
        let mut n = 2;
        while seen.contains(&id) {
            id = format!("{base}-{n}");
            n += 1;
        }
        seen.push(id.clone());
        p.id = id;
        out.push(p);
    }
    Ok(out)
}

/// Model names from an OpenAI-style `/models` answer (`{data: [{id}]}`) or Ollama's
/// `/api/tags` (`{models: [{name}]}`).
pub fn parse_model_list(v: &serde_json::Value) -> Vec<String> {
    let from = |arr: &serde_json::Value, key: &str| -> Vec<String> {
        arr.as_array()
            .map(|a| {
                a.iter().filter_map(|m| m[key].as_str().or_else(|| m["model"].as_str())).map(str::to_owned).collect()
            })
            .unwrap_or_default()
    };
    let mut out = from(&v["data"], "id");
    if out.is_empty() {
        out = from(&v["models"], "name");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(kind: ProviderKind, url: &str) -> AiProvider {
        AiProvider { id: "x".into(), kind, base_url: url.into(), model: "m".into(), ..Default::default() }
    }

    #[test]
    fn endpoints_per_kind() {
        let o = p(ProviderKind::Ollama, "http://localhost:11434/v1");
        assert_eq!(o.chat_url(), "http://localhost:11434/v1/chat/completions");
        assert_eq!(o.models_url().unwrap(), "http://localhost:11434/api/tags");
        let l = p(ProviderKind::LmStudio, LM_STUDIO_URL);
        assert_eq!(l.chat_url(), "http://localhost:1234/v1/chat/completions");
        assert_eq!(l.models_url().unwrap(), "http://localhost:1234/v1/models");
        let a = p(ProviderKind::Anthropic, "https://api.anthropic.com/v1/");
        assert_eq!(a.chat_url(), "https://api.anthropic.com/v1/chat/completions");
        let az =
            AiProvider { model: "gpt 4o".into(), ..p(ProviderKind::Azure, "https://firma.openai.azure.com/openai/") };
        assert_eq!(
            az.chat_url(),
            format!(
                "https://firma.openai.azure.com/openai/deployments/gpt%204o/chat/completions?api-version={AZURE_API_VERSION}"
            )
        );
        assert_eq!(az.models_url(), None);
    }

    #[test]
    fn local_providers_need_no_key_and_say_where_text_goes() {
        let o = AiProvider::new(ProviderKind::Ollama);
        assert!(o.local() && !o.needs_key());
        assert_eq!(o.host(), "localhost");
        let c = p(ProviderKind::Compatible, "http://127.0.0.1:8080/v1");
        assert!(c.local() && !c.needs_key());
        let m = AiProvider::new(ProviderKind::Mistral);
        assert!(!m.local() && m.needs_key());
        assert_eq!(m.host(), "api.mistral.ai");
    }

    #[test]
    fn key_headers() {
        let az = p(ProviderKind::Azure, "https://x.openai.azure.com");
        assert_eq!(az.headers(Some("k1")), [("api-key", "k1".to_string())]);
        let an = p(ProviderKind::Anthropic, "https://api.anthropic.com/v1");
        let h = an.headers(Some("k2"));
        assert!(
            h.contains(&("x-api-key", "k2".into())) && h.contains(&("anthropic-version", ANTHROPIC_VERSION.into()))
        );
        let o = p(ProviderKind::Openai, "https://api.openai.com/v1");
        assert_eq!(o.headers(Some(" k3 ")), [("authorization", "Bearer k3".to_string())]);
        assert!(o.headers(Some("")).is_empty());
    }

    #[test]
    fn normalize_ids_names_and_addresses() {
        let got = normalize(vec![
            AiProvider { name: " Mistral AI ".into(), kind: ProviderKind::Mistral, ..Default::default() },
            AiProvider { name: "Mistral AI".into(), kind: ProviderKind::Mistral, ..Default::default() },
            AiProvider { kind: ProviderKind::Ollama, ..Default::default() },
        ])
        .unwrap();
        let ids: Vec<&str> = got.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["mistral-ai", "mistral-ai-2", "ollama"]);
        assert_eq!(got[0].base_url, "https://api.mistral.ai/v1");
        assert_eq!((got[2].name.as_str(), got[2].base_url.as_str()), ("Ollama", OLLAMA_URL));
        // Plain HTTP only to this computer, as every connection of the app.
        let lan = AiProvider { name: "GPU".into(), base_url: "http://gpu-server:11434".into(), ..Default::default() };
        assert!(normalize(vec![lan]).unwrap_err().to_string().contains("“GPU”"));
        assert_eq!(slug("Größe & Straße"), "grosse-strasse");
    }

    #[test]
    fn unknown_kinds_load_as_compatible_and_model_lists_parse() {
        let p: AiProvider =
            serde_json::from_str(r#"{"id":"a","kind":"gemini-native","base_url":"https://x"}"#).unwrap();
        assert_eq!(p.kind, ProviderKind::Compatible);
        let openai = serde_json::json!({"data":[{"id":"gpt-4o"},{"id":"o4-mini"}]});
        assert_eq!(parse_model_list(&openai), ["gpt-4o", "o4-mini"]);
        let tags = serde_json::json!({"models":[{"name":"llama3.2:latest"},{"model":"qwen3"}]});
        assert_eq!(parse_model_list(&tags), ["llama3.2:latest", "qwen3"]);
    }
}
