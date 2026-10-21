//! Accessibility checks at the session and tool execution boundaries.

use codex_config::output_filter::global;
use codex_config::output_filter::OutputFilter;
use codex_protocol::protocol::EventMsg;
use codex_tools::ToolName;
use codex_tools::ToolPayload;

use crate::function_tool::FunctionCallError;
use crate::tools::context::ToolInvocation;

pub(crate) fn event(message: EventMsg) -> Option<EventMsg> {
    let Some(filter) = global() else { return Some(message); };
    filtered_event(message, filter)
}

fn filtered_event(message: EventMsg, filter: &OutputFilter) -> Option<EventMsg> {
    let kind = message.to_string();
    // The completed item carries the whole text. Unbounded regexes cannot be
    // safely evaluated against independently delivered streaming fragments.
    if kind.ends_with("_delta")
        || kind == "patch_apply_updated"
        || kind.starts_with("realtime_conversation_")
    {
        return None;
    }
    filter.typed(&message)
}

/// Do not silently edit executable arguments. A match asks the model to rephrase.
/// Shells, code execution, plugins, and external tools can synthesize bytes that
/// never pass through the native patch validator, so they are unavailable here.
pub(crate) fn check_tool(invocation: &ToolInvocation) -> Result<(), FunctionCallError> {
    let Some(filter) = global() else { return Ok(()); };
    validate_tool(&invocation.tool_name, &invocation.payload, filter)
}

fn validate_tool(name: &ToolName, payload: &ToolPayload, filter: &OutputFilter) -> Result<(), FunctionCallError> {
    if !name.is_default_namespace()
        || !matches!(name.name.as_str(),
            "apply_patch" | "read_file" | "list_dir" | "grep_files"
            | "update_plan" | "request_user_input")
    {
        return Err(FunctionCallError::RespondToModel(
            "The global output filter permits native validated patches and audited text tools only. Shell, code execution, external tools, and media tools are unavailable in this mode.".to_owned(),
        ));
    }
    let matches = match payload {
        ToolPayload::Function { arguments } => {
            // Decode JSON first so JSON escapes cannot bypass content checks.
            let value = serde_json::from_str(arguments).map_err(|_| {
                FunctionCallError::RespondToModel("Invalid tool input.".to_owned())
            })?;
            filter.contains_json(&value)
        }
        ToolPayload::Custom { input } => filter.is_match(input),
        ToolPayload::ToolSearch { .. } => true,
    };
    if matches {
        return Err(FunctionCallError::RespondToModel(
            "The global output filter blocked this tool input. Rephrase it without quoting the matched text.".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_config::output_filter::OutputFilterConfig;
    use codex_config::output_filter::OutputFilterRule;
    use codex_protocol::protocol::AgentMessageEvent;
    use pretty_assertions::assert_eq;

    fn filter() -> OutputFilter {
        OutputFilter::compile(OutputFilterConfig {
            rules: vec![OutputFilterRule {
                pattern: "(?i)secret".into(), exempt_pattern: None, replacement: "[filtered]".into(),
            }],
        }).unwrap()
    }

    #[test]
    fn complete_message_keeps_its_wire_shape() {
        let filter = filter();
        let original = EventMsg::AgentMessage(AgentMessageEvent {
            message: "A SECRET example".into(), phase: None,
            memory_citation: None, delivery: None, questions: None,
        });
        let filtered = filtered_event(original, &filter).unwrap();
        let EventMsg::AgentMessage(filtered) = filtered else { panic!("wrong event"); };
        assert_eq!(filtered.message, "A [filtered] example");
    }

    #[test]
    fn streaming_fragments_are_withheld_even_when_they_do_not_match_alone() {
        use codex_protocol::protocol::AgentMessageContentDeltaEvent;
        for delta in ["sec", "ret"] {
            let event = EventMsg::AgentMessageContentDelta(AgentMessageContentDeltaEvent {
                thread_id: "thread".into(), turn_id: "turn".into(), item_id: "item".into(), delta: delta.into(),
            });
            assert!(filtered_event(event, &filter()).is_none());
        }
    }

    #[test]
    fn tool_checks_decode_json_and_block_indirect_writers() {
        let payload = ToolPayload::Function { arguments: r#"{"content":"\u0073ecret"}"#.into() };
        assert!(validate_tool(&ToolName::plain("apply_patch"), &payload, &filter()).is_err());
        let safe = ToolPayload::Function { arguments: "{}".into() };
        assert!(validate_tool(&ToolName::plain("exec_command"), &safe, &filter()).is_err());
        assert!(validate_tool(&ToolName::plain("read_file"), &safe, &filter()).is_ok());
        let custom = ToolPayload::Custom { input: "secret".into() };
        assert!(validate_tool(&ToolName::plain("apply_patch"), &custom, &filter()).is_err());
    }
}
