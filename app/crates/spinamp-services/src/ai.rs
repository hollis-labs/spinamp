//! Optional AI: turns a free-text request into a structured `Intent`. Disabled unless a key
//! is configured (env `SPINAMP_AI_KEY` or `ANTHROPIC_API_KEY`). It only ever sees the prompt
//! and the list of tag names, never track titles or listening history.

use serde_json::{Value, json};
use spinamp_core::generate::Intent;

pub const DEFAULT_MODEL: &str = "claude-haiku-5-5";

pub struct Ai {
    key: String,
    model: String,
    http: reqwest::Client,
}

impl Ai {
    pub fn new(key: String, model: String) -> Self {
        let model = if model.trim().is_empty() { DEFAULT_MODEL.into() } else { model };
        Self { key, model, http: reqwest::Client::new() }
    }

    /// Env fallback for people who prefer not to store a key in the app.
    pub fn env_key() -> Option<String> {
        std::env::var("SPINAMP_AI_KEY").or_else(|_| std::env::var("ANTHROPIC_API_KEY")).ok().filter(|k| !k.is_empty())
    }

    /// Cheapest possible request, to check the key works.
    pub async fn ping(&self) -> Result<(), String> {
        let r = self
            .http
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.key)
            .header("anthropic-version", "2023-06-01")
            .json(&json!({"model": self.model, "max_tokens": 8, "messages": [{"role": "user", "content": "ping"}]}))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if r.status().is_success() {
            return Ok(());
        }
        let status = r.status();
        let v: Value = r.json().await.unwrap_or(Value::Null);
        Err(format!("{status}: {}", v["error"]["message"].as_str().unwrap_or("request failed")))
    }

    pub async fn interpret(&self, prompt: &str, vocab: &[String]) -> Result<Intent, String> {
        let system = "You convert a music playlist request into JSON for a playlist generator. \
Output ONLY one JSON object with keys: name (short playlist title, <= 40 chars), target_minutes (number or null; \
set only if the user gave or clearly implied a length), include_tags (array chosen ONLY from VOCAB), \
exclude_tags (array chosen ONLY from VOCAB), max_per_artist (integer or null), strict (boolean; true only if the \
user insisted on a genre), tempo_min / tempo_max (BPM or null), energy_min / energy_max (0..1 or null; 0.3 is quiet, \
0.5 moderate, 0.7+ intense), moods (array chosen ONLY from: uplifting, energetic, aggressive, dark, calm, peaceful, melancholic, somber, dreamy, danceable, upbeat, moody, balanced; empty if the request implies none), max_spike_db (number or null; set to about 6 when the user wants steady volume or no \
sudden loud moments). Pick 3-8 include_tags that fit the mood, activity or genre, and set tempo/energy only when the \
request implies them (e.g. workout: tempo_min 120, energy_min 0.6; sleep: energy_max 0.35).";
        let user = format!("VOCAB: {}\n\nREQUEST: {}", vocab.join(", "), prompt);
        let r = self
            .http
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.key)
            .header("anthropic-version", "2023-06-01")
            .json(&json!({"model": self.model, "max_tokens": 500, "system": system,
                          "messages": [{"role": "user", "content": user}]}))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let status = r.status();
        let v: Value = r.json().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("ai {status}: {}", v["error"]["message"].as_str().unwrap_or("request failed")));
        }
        let text = v["content"][0]["text"].as_str().ok_or("empty AI response")?;
        parse_intent(text)
    }
}

/// Models sometimes wrap JSON in prose or code fences; take the outermost object.
pub fn parse_intent(text: &str) -> Result<Intent, String> {
    let (a, b) = (text.find('{').ok_or("no JSON in AI response")?, text.rfind('}').ok_or("no JSON in AI response")?);
    serde_json::from_str(&text[a..=b]).map_err(|e| format!("bad AI JSON: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fenced_and_partial_json() {
        let i = parse_intent("Sure!\n```json\n{\"name\":\"Commute\",\"target_minutes\":45,\"include_tags\":[\"indie rock\"]}\n```").unwrap();
        assert_eq!((i.name.as_str(), i.target_minutes, i.include_tags.len()), ("Commute", Some(45.0), 1));
        assert!(i.exclude_tags.is_empty() && !i.strict);
        assert!(parse_intent("no json here").is_err());
    }
}
