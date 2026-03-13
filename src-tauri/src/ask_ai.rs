use anyhow::{anyhow, Result};

use crate::database::Database;
use crate::summarization;
use crate::types::{AskAiResponse, MeetingReference};

pub async fn ask_ai(
    question: &str,
    meeting_id: Option<&str>,
    database: &Database,
    api_key: &str,
    provider: &str,
    model: &str,
) -> Result<AskAiResponse> {
    if api_key.is_empty() {
        return Err(anyhow!(
            "No LLM API key configured. Add your API key in Settings to enable Ask AI."
        ));
    }

    // Search for relevant transcript segments using FTS5
    let search_results = database.search_transcripts(question)?;

    // Collect unique meeting IDs from search results (up to 5)
    let mut seen_meeting_ids: Vec<String> = Vec::new();
    for result in &search_results {
        if !seen_meeting_ids.contains(&result.meeting_id) {
            seen_meeting_ids.push(result.meeting_id.clone());
            if seen_meeting_ids.len() >= 5 {
                break;
            }
        }
    }

    // If scoped to a single meeting, restrict to that meeting only
    let relevant_meeting_ids: Vec<String> = if let Some(id) = meeting_id {
        if seen_meeting_ids.contains(&id.to_string()) {
            vec![id.to_string()]
        } else {
            // No FTS hits for this meeting - still include it for context
            vec![id.to_string()]
        }
    } else {
        seen_meeting_ids
    };

    // Build context sections for each relevant meeting
    let mut context_sections: Vec<String> = Vec::new();
    let mut meeting_references: Vec<MeetingReference> = Vec::new();

    for mid in &relevant_meeting_ids {
        let detail = match database.get_meeting_detail(mid) {
            Ok(d) => d,
            Err(_) => continue,
        };

        let title = &detail.summary.title;
        let date = detail
            .summary
            .started_at
            .split('T')
            .next()
            .unwrap_or(&detail.summary.started_at);

        let mut section = format!("### Meeting: \"{title}\" ({date})\n");

        if let Some(summary_text) = &detail.summary_text {
            if !summary_text.is_empty() {
                section.push_str(&format!("Summary: {summary_text}\n"));
            }
        }

        // Collect relevant excerpts for this meeting from search results
        let excerpts: Vec<String> = search_results
            .iter()
            .filter(|r| &r.meeting_id == mid)
            .take(5)
            .map(|r| {
                let timestamp = format_timestamp(r.start_time);
                format!("- [{timestamp}] \"{}\"", r.snippet)
            })
            .collect();

        if !excerpts.is_empty() {
            section.push_str("Relevant excerpts:\n");
            for excerpt in &excerpts {
                section.push_str(&format!("{excerpt}\n"));
            }
        }

        // Build the MeetingReference for the response
        let first_excerpt = search_results
            .iter()
            .find(|r| &r.meeting_id == mid)
            .map(|r| r.snippet.clone())
            .unwrap_or_default();

        meeting_references.push(MeetingReference {
            meeting_id: mid.clone(),
            meeting_title: title.clone(),
            relevant_excerpt: first_excerpt,
        });

        context_sections.push(section);
    }

    let context = if context_sections.is_empty() {
        "No meeting content found matching your question.".to_string()
    } else {
        context_sections.join("\n")
    };

    let prompt = format!(
        "You are an AI assistant with access to meeting transcripts and summaries. \
Answer the user's question based on the meeting context provided below.\n\n\
When referencing information, mention which meeting it came from.\n\n\
If the context doesn't contain enough information to answer the question, say so clearly.\n\n\
## Meeting Context\n\n\
{context}\n\n\
## User Question\n\
{question}\n\n\
Respond in a helpful, concise manner. Reference specific meetings when citing information."
    );

    let answer = match provider {
        "openai" => summarization::call_openai(api_key, model, &prompt).await?,
        _ => summarization::call_anthropic(api_key, model, &prompt).await?,
    };

    Ok(AskAiResponse {
        answer,
        meeting_references,
    })
}

fn format_timestamp(seconds: f64) -> String {
    let total = seconds as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}
