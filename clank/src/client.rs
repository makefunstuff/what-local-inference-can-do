//! Blocking OpenAI-compatible SSE chat client (ureq).

use std::io::{BufRead, Read};
use serde_json::{json, Value};
use crate::Fail;

/// One streamed chat round: text deltas plus any tool calls.
pub struct Round {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
}

pub struct ToolCall {
    pub id: Option<String>,
    pub name: String,
    /// raw JSON string, accumulated from streamed fragments
    pub arguments: String,
}

pub fn stream_round(
    agent: &ureq::Agent,
    base_url: &str,
    model: &str,
    api_key: Option<&str>,
    max_tokens: u32,
    messages: &[Value],
    tools: &Value,
    emit: &mut dyn FnMut(&str),
    debug_path: Option<&str>,
) -> Result<Round, Fail> {
    let url = format!("{base_url}/chat/completions");
    let body = json!({
        "model": model,
        "messages": messages,
        "tools": tools,
        "stream": true,
        "max_tokens": max_tokens,
    });
    if let Some(p) = debug_path {
        let _ = std::fs::write(p, serde_json::to_string_pretty(&body).unwrap_or_default());
    }

    let mut req = agent.post(&url).header("Content-Type", "application/json");
    if let Some(key) = api_key {
        req = req.header("Authorization", format!("Bearer {key}"));
    }
    let mut resp = req
        .send_json(&body)
        .map_err(|e| Fail::Model(format!("request to {url} failed: {e}")))?;

    if resp.status() != 200 {
        let mut detail = String::new();
        let _ = resp.body_mut().as_reader().read_to_string(&mut detail);
        return Err(Fail::Model(format!(
            "model returned HTTP {status}: {detail}",
            status = resp.status()
        )));
    }

    let mut round = Round {
        text: String::new(),
        tool_calls: Vec::new(),
        finish_reason: None,
    };
    let lines = std::io::BufReader::new(resp.body_mut().as_reader()).lines();

    for line in lines {
        let line = line.map_err(|e| Fail::Model(format!("stream read error: {e}")))?;
        let Some(data) = line.strip_prefix("data:") else { continue };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            if data == "[DONE]" {
                break;
            }
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else { continue };
        let Some(choices) = v.get("choices").and_then(|c| c.as_array()) else { continue };
        for ch in choices {
            if let Some(fr) = ch.get("finish_reason").and_then(|f| f.as_str()) {
                round.finish_reason = Some(fr.to_string());
            }
            let Some(delta) = ch.get("delta") else { continue };
            if let Some(t) = delta.get("content").and_then(|c| c.as_str()) {
                // `reasoning_content` is deliberately not emitted: stdout is data only.
                round.text.push_str(t);
                emit(t);
            }
            if let Some(tcs) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                for tc in tcs {
                    let Some(idx) = tc.get("index").and_then(|i| i.as_u64()) else { continue };
                    while round.tool_calls.len() <= idx as usize {
                        round.tool_calls.push(ToolCall {
                            id: None,
                            name: String::new(),
                            arguments: String::new(),
                        });
                    }
                    let slot = &mut round.tool_calls[idx as usize];
                    if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                        slot.id = Some(id.to_string());
                    }
                    let fn_ = tc.get("function");
                    if let Some(name) = fn_.and_then(|f| f.get("name")).and_then(|n| n.as_str()) {
                        slot.name = name.to_string();
                    }
                    if let Some(args) = fn_.and_then(|f| f.get("arguments")).and_then(|a| a.as_str()) {
                        slot.arguments.push_str(args);
                    }
                }
            }
        }
    }

    Ok(round)
}
