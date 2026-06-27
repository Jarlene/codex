use crate::model::MessageId;
use crate::model::MessageSummary;
use crate::model::SummaryContent;
use crate::model::SummaryId;
use crate::model::TeamMessage;

pub fn summarize_messages(
    summary_id: SummaryId,
    source_messages: &[MessageId],
    messages: &[TeamMessage],
    compressed_at: i64,
) -> MessageSummary {
    let mut content = SummaryContent::default();
    for message in messages {
        let text = message.payload.content.trim();
        if text.is_empty() {
            continue;
        }
        classify_line(text, &mut content);
    }
    if content.context_for_lead.is_empty() {
        content.context_for_lead = messages
            .iter()
            .map(|message| {
                format!(
                    "{}: {}",
                    message.from.0,
                    truncate(&message.payload.content, 240)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
    }

    MessageSummary {
        id: summary_id,
        source_messages: source_messages.to_vec(),
        compressed_at,
        content,
    }
}

fn classify_line(text: &str, content: &mut SummaryContent) {
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        if contains_any(&lower, &["decision:", "decided", "choose", "selected"])
            || trimmed.contains("决定")
            || trimmed.contains("选择")
        {
            push_limited(&mut content.key_decisions, cleaned(trimmed), 8);
        } else if contains_any(&lower, &["risk:", "risk ", "compatibility", "unsafe"])
            || trimmed.contains("风险")
        {
            push_limited(&mut content.risks, cleaned(trimmed), 8);
        } else if contains_any(&lower, &["blocker:", "blocked", "waiting on"])
            || trimmed.contains("阻塞")
        {
            push_limited(&mut content.blockers, cleaned(trimmed), 8);
        } else if contains_any(&lower, &["action:", "todo:", "next:", "follow up"])
            || trimmed.contains("行动项")
        {
            push_limited(&mut content.action_items, cleaned(trimmed), 8);
        }
    }
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

fn push_limited(items: &mut Vec<String>, item: String, max_items: usize) {
    if items.len() < max_items && !items.contains(&item) {
        items.push(item);
    }
}

fn cleaned(line: &str) -> String {
    let line = line.trim_start_matches('-').trim_start_matches('*').trim();
    truncate(line, 280)
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut truncated = text.chars().take(max_chars).collect::<String>();
    truncated.push_str("...");
    truncated
}
