use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::types::{SummaryResult, TaskExtraction};

// ---- Anthropic request/response shapes ----

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<AnthropicMessage>,
}

#[derive(Serialize)]
struct AnthropicMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContentBlock>,
}

#[derive(Deserialize)]
struct AnthropicContentBlock {
    text: String,
}

// ---- OpenAI request/response shapes ----

#[derive(Serialize)]
struct OpenAiRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
}

#[derive(Serialize)]
struct OpenAiMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct OpenAiResponse {
    choices: Vec<OpenAiChoice>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessageContent,
}

#[derive(Deserialize)]
struct OpenAiMessageContent {
    content: String,
}

// ---- Raw LLM JSON output ----

#[derive(Deserialize)]
struct LlmOutput {
    summary: String,
    tasks: Vec<LlmTask>,
}

#[derive(Deserialize)]
struct LlmTask {
    text: String,
    assignee: Option<String>,
}

// ---- Public API ----

pub(crate) async fn call_anthropic(api_key: &str, model: &str, prompt: &str) -> Result<String> {
    let client = reqwest::Client::new();
    let request_body = AnthropicRequest {
        model: model.to_string(),
        max_tokens: 4096,
        messages: vec![AnthropicMessage {
            role: "user".to_string(),
            content: prompt.to_string(),
        }],
    };

    let response = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&request_body)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("Anthropic API error {status}: {body}"));
    }

    let parsed: AnthropicResponse = response.json().await?;
    parsed
        .content
        .into_iter()
        .next()
        .map(|block| block.text)
        .ok_or_else(|| anyhow!("Anthropic API returned an empty response"))
}

pub(crate) async fn call_openai(api_key: &str, model: &str, prompt: &str) -> Result<String> {
    let client = reqwest::Client::new();
    let request_body = OpenAiRequest {
        model: model.to_string(),
        messages: vec![OpenAiMessage {
            role: "user".to_string(),
            content: prompt.to_string(),
        }],
    };

    let response = client
        .post("https://api.openai.com/v1/chat/completions")
        .header("authorization", format!("Bearer {api_key}"))
        .header("content-type", "application/json")
        .json(&request_body)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("OpenAI API error {status}: {body}"));
    }

    let parsed: OpenAiResponse = response.json().await?;
    parsed
        .choices
        .into_iter()
        .next()
        .and_then(|choice| Some(choice.message.content))
        .ok_or_else(|| anyhow!("OpenAI API returned an empty response"))
}

fn parse_llm_output(raw: &str) -> Result<SummaryResult> {
    // Strip markdown code fences if present
    let trimmed = raw.trim();
    let json_str = if let Some(inner) = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
    {
        inner.trim_end_matches("```").trim()
    } else {
        trimmed
    };

    let output: LlmOutput = serde_json::from_str(json_str)
        .map_err(|e| anyhow!("Failed to parse LLM JSON output: {e}\nRaw: {json_str}"))?;

    Ok(SummaryResult {
        summary: output.summary,
        tasks: output
            .tasks
            .into_iter()
            .map(|t| TaskExtraction {
                text: t.text,
                assignee: t.assignee,
            })
            .collect(),
    })
}

pub async fn generate_summary(
    transcript_text: &str,
    prompt_template: &str,
    api_key: &str,
    provider: &str,
    model: &str,
) -> Result<SummaryResult> {
    if api_key.is_empty() {
        return Err(anyhow!(
            "No LLM API key configured. Add your API key in Settings to enable AI summaries."
        ));
    }

    let prompt = format!("{prompt_template}{transcript_text}");

    let raw = match provider {
        "openai" => call_openai(api_key, model, &prompt).await?,
        _ => call_anthropic(api_key, model, &prompt).await?,
    };

    parse_llm_output(&raw)
}
