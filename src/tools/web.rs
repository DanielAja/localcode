//! Web search + fetch tools — the agent's "search the web on the user's behalf"
//! capability. Keyless DuckDuckGo HTML search + HTML→readable-text extraction.
//!
//! These run in-process (trusted) rather than via the network-denied bash sandbox,
//! so they are the sanctioned way for the model to reach the internet.

use super::{arg_str, arg_str_opt, arg_u64_opt, Tool, ToolContext, ToolOutput};
use crate::Result;
use anyhow::{anyhow, Context};
use scraper::{Html, Selector};
use serde_json::{json, Value};
use std::time::Duration;

const UA: &str = "Mozilla/5.0 (localcode; +local)";

/// Blocking HTTP GET on a dedicated thread (avoids nested-tokio-runtime panic).
fn blocking_get(url: String) -> Result<String> {
    std::thread::spawn(move || -> Result<String> {
        let client = reqwest::blocking::Client::builder()
            .user_agent(UA)
            .timeout(Duration::from_secs(20))
            .build()?;
        let resp = client.get(&url).send()?;
        let status = resp.status();
        if !status.is_success() {
            return Err(anyhow!("HTTP {status}"));
        }
        Ok(resp.text()?)
    })
    .join()
    .map_err(|_| anyhow!("web request thread panicked"))?
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut i = max;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    format!("{}…", &s[..i])
}

// ---------- web_search ----------

pub struct WebSearchTool;

impl Tool for WebSearchTool {
    fn name(&self) -> &'static str {
        "web_search"
    }
    fn description(&self) -> &'static str {
        "Search the web (DuckDuckGo) and return the top results as title + URL + snippet. Use this to find current information, documentation, or examples, then read a result with web_fetch."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "max_results": {"type": "integer", "description": "1-10 (default 5)"}
            },
            "required": ["query"]
        })
    }
    fn mutating(&self) -> bool {
        false
    }
    fn summary(&self, args: &Value) -> String {
        format!("web search: {}", arg_str_opt(args, "query").unwrap_or("?"))
    }
    fn run(&self, args: &Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let query = arg_str(args, "query")?;
        let max = arg_u64_opt(args, "max_results").unwrap_or(5).clamp(1, 10) as usize;
        let url = format!("https://html.duckduckgo.com/html/?q={}", urlencoding::encode(query));
        let html = blocking_get(url).context("web search request")?;
        let results = parse_ddg(&html, max);
        if results.trim().is_empty() {
            Ok(ToolOutput::ok("(no results)"))
        } else {
            Ok(ToolOutput::ok(results))
        }
    }
}

fn parse_ddg(html: &str, max: usize) -> String {
    let doc = Html::parse_document(html);
    let res_sel = Selector::parse("div.result").unwrap();
    let a_sel = Selector::parse("a.result__a").unwrap();
    let snip_sel = Selector::parse("a.result__snippet, .result__snippet").unwrap();
    let mut out = String::new();
    let mut n = 0;
    for r in doc.select(&res_sel) {
        let Some(a) = r.select(&a_sel).next() else { continue };
        let title = a.text().collect::<String>().trim().to_string();
        if title.is_empty() {
            continue;
        }
        let href = a.value().attr("href").unwrap_or("");
        let url = decode_ddg_href(href);
        let snippet = r
            .select(&snip_sel)
            .next()
            .map(|s| s.text().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_default();
        n += 1;
        out.push_str(&format!("{n}. {title}\n   {url}\n"));
        if !snippet.is_empty() {
            out.push_str(&format!("   {}\n", truncate(&snippet, 240)));
        }
        if n >= max {
            break;
        }
    }
    out
}

/// DuckDuckGo wraps result links as `//duckduckgo.com/l/?uddg=<encoded>&...`.
fn decode_ddg_href(href: &str) -> String {
    if let Some(idx) = href.find("uddg=") {
        let rest = &href[idx + 5..];
        let enc = rest.split('&').next().unwrap_or(rest);
        if let Ok(dec) = urlencoding::decode(enc) {
            return dec.into_owned();
        }
    }
    if href.starts_with("http") {
        href.to_string()
    } else if let Some(stripped) = href.strip_prefix("//") {
        format!("https://{stripped}")
    } else {
        href.to_string()
    }
}

// ---------- web_fetch ----------

pub struct WebFetchTool;

impl Tool for WebFetchTool {
    fn name(&self) -> &'static str {
        "web_fetch"
    }
    fn description(&self) -> &'static str {
        "Fetch a URL and return its readable text content (HTML stripped). Use after web_search to read a page."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {"url": {"type": "string"}},
            "required": ["url"]
        })
    }
    fn mutating(&self) -> bool {
        false
    }
    fn summary(&self, args: &Value) -> String {
        format!("web fetch: {}", arg_str_opt(args, "url").unwrap_or("?"))
    }
    fn run(&self, args: &Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let url = arg_str(args, "url")?;
        let url = if url.starts_with("http") { url.to_string() } else { format!("https://{url}") };
        let html = blocking_get(url).context("web fetch request")?;
        let text = html_to_text(&html);
        if text.trim().is_empty() {
            Ok(ToolOutput::ok("(no readable text extracted)"))
        } else {
            Ok(ToolOutput::ok(truncate(&text, 8000)))
        }
    }
}

fn html_to_text(html: &str) -> String {
    let doc = Html::parse_document(html);
    let sel = Selector::parse("h1,h2,h3,h4,h5,h6,p,li,pre,blockquote,td").unwrap();
    let mut out = String::new();
    for el in doc.select(&sel) {
        let t = el.text().collect::<String>();
        let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
        if !t.is_empty() {
            out.push_str(&t);
            out.push('\n');
        }
    }
    if out.trim().is_empty() {
        if let Some(body) = doc.select(&Selector::parse("body").unwrap()).next() {
            out = body.text().collect::<Vec<_>>().join(" ");
            out = out.split_whitespace().collect::<Vec<_>>().join(" ");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ToolContext {
        ToolContext {
            workspace: std::path::PathBuf::from("."),
            bash_timeout: Duration::from_secs(10),
            sandbox: crate::config::SandboxLevel::WorkspaceWrite,
        }
    }

    #[test]
    #[ignore = "requires network"]
    fn web_search_returns_results() {
        let args = serde_json::json!({"query": "rust programming language", "max_results": 3});
        let out = WebSearchTool.run(&args, &ctx()).unwrap();
        assert!(!out.is_error, "got error: {}", out.content);
        assert!(out.content.contains("http"), "expected URLs:\n{}", out.content);
    }

    #[test]
    #[ignore = "requires network"]
    fn web_fetch_extracts_text() {
        let args = serde_json::json!({"url": "https://example.com"});
        let out = WebFetchTool.run(&args, &ctx()).unwrap();
        assert!(!out.is_error, "got error: {}", out.content);
        assert!(out.content.to_lowercase().contains("example"), "expected page text:\n{}", out.content);
    }
}

