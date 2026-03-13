use anyhow::Result;

use crate::types::MeetingDetail;

pub fn export_meeting(detail: &MeetingDetail, format: &str) -> Result<String> {
    match format {
        "md" => Ok(to_markdown(detail)),
        "txt" => Ok(to_text(detail)),
        "srt" => Ok(to_srt(detail)),
        "json" => Ok(serde_json::to_string_pretty(detail)?),
        "html" => Ok(to_html(detail)),
        _ => Ok(to_markdown(detail)),
    }
}

fn to_html(detail: &MeetingDetail) -> String {
    let summary_html = if let Some(summary) = &detail.summary_text {
        crate::database::markdown_to_html_public(summary)
    } else {
        String::new()
    };

    let transcript_rows: String = detail
        .transcript_segments
        .iter()
        .map(|s| {
            let minutes = (s.start_time / 60.0) as u64;
            let secs = (s.start_time % 60.0) as u64;
            let speaker = s.speaker.as_deref().unwrap_or("Speaker");
            format!(
                "<tr><td style=\"color:#6B7280;white-space:nowrap\">[{minutes:02}:{secs:02}]</td><td><strong>{}</strong></td><td>{}</td></tr>\n",
                html_escape_export(speaker),
                html_escape_export(&s.text)
            )
        })
        .collect();

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<title>{title}</title>
<style>
body {{ font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif; max-width: 900px; margin: 40px auto; padding: 0 20px; color: #111; }}
h1, h2, h3 {{ color: #111; }}
table {{ width: 100%; border-collapse: collapse; font-size: 0.9em; }}
td {{ padding: 4px 8px; vertical-align: top; }}
</style>
</head>
<body>
<h1>{title}</h1>
<p style="color:#6B7280">Started: {started_at} &middot; Duration: {duration}s &middot; Platform: {platform}</p>
{summary_section}
<h2>Transcript</h2>
<table>
<tbody>
{transcript_rows}
</tbody>
</table>
</body>
</html>"#,
        title = html_escape_export(&detail.summary.title),
        started_at = detail.summary.started_at,
        duration = detail.summary.duration_seconds,
        platform = detail.summary.platform,
        summary_section = if summary_html.is_empty() {
            String::new()
        } else {
            format!("<h2>Summary</h2>\n{summary_html}")
        },
        transcript_rows = transcript_rows,
    )
}

fn html_escape_export(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn to_markdown(detail: &MeetingDetail) -> String {
    let mut output = format!(
        "# {}\n\n- Started: {}\n- Duration: {} seconds\n- Platform: {}\n\n## Transcript\n\n",
        detail.summary.title,
        detail.summary.started_at,
        detail.summary.duration_seconds,
        detail.summary.platform
    );
    for segment in &detail.transcript_segments {
        output.push_str(&format!(
            "- [{} - {}] {}: {}\n",
            segment.start_time,
            segment.end_time,
            segment.speaker.as_deref().unwrap_or("Speaker"),
            segment.text
        ));
    }
    output
}

fn to_text(detail: &MeetingDetail) -> String {
    detail
        .transcript_segments
        .iter()
        .map(|segment| {
            format!(
                "{}: {}",
                segment.speaker.as_deref().unwrap_or("Speaker"),
                segment.text
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn to_srt(detail: &MeetingDetail) -> String {
    detail
        .transcript_segments
        .iter()
        .enumerate()
        .map(|(index, segment)| {
            format!(
                "{}\n{} --> {}\n{}\n",
                index + 1,
                to_srt_time(segment.start_time),
                to_srt_time(segment.end_time),
                segment.text
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn to_srt_time(seconds: f64) -> String {
    let total_millis = (seconds * 1000.0).round() as u64;
    let millis = total_millis % 1000;
    let total_seconds = total_millis / 1000;
    let secs = total_seconds % 60;
    let minutes = (total_seconds / 60) % 60;
    let hours = total_seconds / 3600;
    format!("{hours:02}:{minutes:02}:{secs:02},{millis:03}")
}

#[cfg(test)]
mod tests {
    use crate::types::{MeetingDetail, MeetingStatus, MeetingSummary, TranscriptSegment};

    use super::to_srt;

    #[test]
    fn srt_export_contains_ranges() {
        let detail = MeetingDetail {
            summary: MeetingSummary {
                id: "1".into(),
                title: "Demo".into(),
                started_at: "2026-03-09T10:00:00Z".into(),
                duration_seconds: 30,
                audio_file_path: None,
                platform: "macOS".into(),
                status: MeetingStatus::Completed,
                segment_count: 1,
                tags: vec![],
                calendar_event_title: None,
            },
            transcript_segments: vec![TranscriptSegment {
                id: "s1".into(),
                meeting_id: "1".into(),
                start_time: 1.2,
                end_time: 3.4,
                text: "Hello".into(),
                speaker: Some("Speaker 1".into()),
                language: "en".into(),
            }],
            jobs: vec![],
            summary_text: None,
            scratchpad: None,
            tasks: vec![],
            speakers: vec![],
        };

        let srt = to_srt(&detail);
        assert!(srt.contains("00:00:01,200 --> 00:00:03,400"));
    }
}
