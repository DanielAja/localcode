//! The inference engine: an OpenAI-compatible HTTP client that talks to a local
//! `llama-server` (which we spawn) or an attached endpoint (`--attach`).
//!
//! Tool-calling reliability lives in `llama-server --jinja` (native per-family
//! handlers + lazy grammars), so the engine speaks the standard `tools` /
//! `tool_calls` schema and reads structured tool calls straight from the response.

pub mod llama_server;
pub mod provision;
pub mod server_install;

use crate::Result;
use anyhow::{anyhow, Context};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// One chat message in OpenAI format. Used both for requests (history) and to
/// deserialize assistant responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    /// Always serialized (as `null` when absent) for OpenAI compatibility.
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub name: Option<String>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self::simple(Role::System, content)
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self::simple(Role::User, content)
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::simple(Role::Assistant, content)
    }
    fn simple(role: Role, content: impl Into<String>) -> Self {
        ChatMessage {
            role,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }
    /// A `role: tool` result message answering a specific tool call.
    pub fn tool_result(tool_call_id: impl Into<String>, name: impl Into<String>, content: impl Into<String>) -> Self {
        ChatMessage {
            role: Role::Tool,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            name: Some(name.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    #[serde(default)]
    pub id: String,
    #[serde(rename = "type", default = "default_function_type")]
    pub call_type: String,
    pub function: FunctionCall,
}

fn default_function_type() -> String {
    "function".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    /// JSON-encoded arguments (a *string*, per the OpenAI schema).
    #[serde(default)]
    pub arguments: String,
}

/// A tool advertised to the model.
#[derive(Debug, Clone, Serialize)]
pub struct ToolSpec {
    #[serde(rename = "type")]
    pub spec_type: String,
    pub function: FunctionSpec,
}

#[derive(Debug, Clone, Serialize)]
pub struct FunctionSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

impl ToolSpec {
    pub fn function(name: impl Into<String>, description: impl Into<String>, parameters: serde_json::Value) -> Self {
        ToolSpec {
            spec_type: "function".to_string(),
            function: FunctionSpec {
                name: name.into(),
                description: description.into(),
                parameters,
            },
        }
    }
}

#[derive(Debug, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<&'a [ToolSpec]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<serde_json::Value>,
    stream: bool,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct ChatResponse {
    #[serde(default)]
    pub choices: Vec<Choice>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
pub struct Choice {
    pub message: ChatMessage,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: u32,
    #[serde(default)]
    pub completion_tokens: u32,
    #[serde(default)]
    pub total_tokens: u32,
}

/// Accumulator for tool-call fragments arriving across streaming deltas.
#[derive(Default)]
struct ToolCallAccum {
    id: String,
    name: String,
    arguments: String,
}

/// The engine: holds an HTTP client + base URL + model name, and optionally owns
/// the `llama-server` child process it spawned.
pub struct Engine {
    client: reqwest::Client,
    base_url: String,
    model: String,
    /// Owned child server (kept alive for the engine's lifetime). `None` when attached.
    _server: Option<llama_server::LlamaServer>,
    temperature: f32,
}

impl Engine {
    /// Build an engine that talks to an already-running endpoint (no child process).
    pub fn attached(base_url: impl Into<String>, model: impl Into<String>, temperature: f32) -> Self {
        Engine {
            client: reqwest::Client::new(),
            base_url: normalize_base(base_url.into()),
            model: model.into(),
            _server: None,
            temperature,
        }
    }

    /// Build an engine that owns a spawned `llama-server`.
    pub fn owning(server: llama_server::LlamaServer, model: impl Into<String>, temperature: f32) -> Self {
        let base_url = server.base_url().to_string();
        Engine {
            client: reqwest::Client::new(),
            base_url,
            model: model.into(),
            _server: Some(server),
            temperature,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// One non-streaming chat completion. Returns the assistant message verbatim
    /// (including any structured `tool_calls`).
    pub async fn chat(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[ToolSpec]>,
        tool_choice: Option<serde_json::Value>,
        max_tokens: Option<u32>,
    ) -> Result<(ChatMessage, Option<Usage>)> {
        let req = ChatRequest {
            model: &self.model,
            messages,
            tools,
            tool_choice,
            stream: false,
            temperature: self.temperature,
            max_tokens,
        };
        let url = format!("{}/v1/chat/completions", self.base_url);
        let resp = self
            .client
            .post(&url)
            .json(&req)
            .timeout(Duration::from_secs(300))
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;

        let status = resp.status();
        let body = resp.text().await.context("reading response body")?;
        if !status.is_success() {
            return Err(anyhow!("llama-server returned {status}: {body}"));
        }
        let parsed: ChatResponse = serde_json::from_str(&body)
            .with_context(|| format!("decoding chat response: {body}"))?;
        let choice = parsed
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("no choices in response: {body}"))?;
        Ok((choice.message, parsed.usage))
    }

    /// Streaming chat completion. Calls `on_delta` for each text fragment as it
    /// arrives, and returns the fully accumulated assistant message (content +
    /// tool_calls) when the stream ends — so the agent loop is unchanged downstream.
    pub async fn chat_stream(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[ToolSpec]>,
        tool_choice: Option<serde_json::Value>,
        max_tokens: Option<u32>,
        mut on_delta: impl FnMut(&str),
    ) -> Result<(ChatMessage, Option<Usage>)> {
        let req = ChatRequest {
            model: &self.model,
            messages,
            tools,
            tool_choice,
            stream: true,
            temperature: self.temperature,
            max_tokens,
        };
        let url = format!("{}/v1/chat/completions", self.base_url);
        let resp = self
            .client
            .post(&url)
            .json(&req)
            .timeout(Duration::from_secs(300))
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("llama-server returned {status}: {body}"));
        }

        let mut content = String::new();
        let mut tool_accum: Vec<ToolCallAccum> = Vec::new();
        let mut buf: Vec<u8> = Vec::new();
        let mut stream = resp.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("reading SSE stream")?;
            buf.extend_from_slice(&chunk);
            // Process complete '\n'-terminated lines ('\n' never appears mid-UTF-8-char).
            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line);
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let data = line.strip_prefix("data:").map(str::trim).unwrap_or(line);
                if data == "[DONE]" {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<serde_json::Value>(data) else {
                    continue;
                };
                let Some(choice) = v.get("choices").and_then(|c| c.get(0)) else {
                    continue;
                };
                let delta = &choice["delta"];
                if let Some(c) = delta.get("content").and_then(|c| c.as_str()) {
                    content.push_str(c);
                    on_delta(c);
                }
                if let Some(tcs) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                    for tc in tcs {
                        let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                        while tool_accum.len() <= idx {
                            tool_accum.push(ToolCallAccum::default());
                        }
                        let acc = &mut tool_accum[idx];
                        if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                            if !id.is_empty() {
                                acc.id = id.to_string();
                            }
                        }
                        if let Some(name) = tc.pointer("/function/name").and_then(|n| n.as_str()) {
                            acc.name.push_str(name);
                        }
                        if let Some(args) = tc.pointer("/function/arguments").and_then(|a| a.as_str()) {
                            acc.arguments.push_str(args);
                        }
                    }
                }
            }
        }

        let tool_calls: Vec<ToolCall> = tool_accum
            .into_iter()
            .filter(|a| !a.name.is_empty())
            .enumerate()
            .map(|(i, a)| ToolCall {
                id: if a.id.is_empty() { format!("call_{i}") } else { a.id },
                call_type: "function".to_string(),
                function: FunctionCall { name: a.name, arguments: a.arguments },
            })
            .collect();

        let msg = ChatMessage {
            role: Role::Assistant,
            content: if content.is_empty() { None } else { Some(content) },
            tool_calls: if tool_calls.is_empty() { None } else { Some(tool_calls) },
            tool_call_id: None,
            name: None,
        };
        Ok((msg, None))
    }

    /// GET /health (llama-server) — returns Ok when the server is ready.
    pub async fn health(&self) -> Result<()> {
        let url = format!("{}/health", self.base_url);
        let resp = self.client.get(&url).timeout(Duration::from_secs(5)).send().await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(anyhow!("health check failed: {}", resp.status()))
        }
    }
}

/// Quick health probe of a base URL (e.g. to reuse an already-running server).
pub async fn ping(base_url: &str) -> bool {
    let url = format!("{}/health", base_url.trim_end_matches('/'));
    matches!(
        reqwest::Client::new().get(&url).timeout(Duration::from_secs(1)).send().await,
        Ok(r) if r.status().is_success()
    )
}

fn normalize_base(mut s: String) -> String {
    while s.ends_with('/') {
        s.pop();
    }
    // Allow `host:port` shorthand.
    if !s.starts_with("http://") && !s.starts_with("https://") {
        s = format!("http://{s}");
    }
    s
}
