use crate::function_tool::FunctionCallError;
use crate::safety::PatchPolicyMatcher;
use crate::safety::SafetyCheck;
use crate::safety::assess_patch_safety;
use crate::session::step_context::StepContext;
use crate::session::turn_context::TurnEnvironment;
use crate::tools::sandboxing::ExecApprovalRequirement;
use codex_apply_patch::ApplyPatchAction;
use codex_apply_patch::ApplyPatchFileChange;
use codex_protocol::protocol::FileChange;
use codex_utils_path_uri::PathUri;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug)]
pub(crate) struct ApplyPatchRuntimeInvocation {
    pub(crate) action: ApplyPatchAction,
    pub(crate) auto_approved: bool,
    pub(crate) exec_approval_requirement: ExecApprovalRequirement,
}

pub(crate) fn prepare_apply_patch(
    step_context: &StepContext,
    turn_environment: &TurnEnvironment,
    matching: &PatchPolicyMatcher<'_>,
    action: ApplyPatchAction,
) -> Result<ApplyPatchRuntimeInvocation, FunctionCallError> {
    if let Some(filter) = codex_config::output_filter::global() {
        for (path, change) in action.changes() {
            let (content, destination) = match change {
                ApplyPatchFileChange::Add { content } => (Some(content), None),
                ApplyPatchFileChange::Update {
                    new_content,
                    move_path,
                    ..
                } => (Some(new_content), move_path.as_ref()),
                ApplyPatchFileChange::Delete { .. } => (None, None),
            };
            if filter.is_match(&path.to_string())
                || destination.is_some_and(|path| filter.is_match(&path.to_string()))
                || content.is_some_and(|content| filter.is_match(content))
            {
                return Err(FunctionCallError::RespondToModel(
                    "The global output filter blocked the resulting file content or path. Rephrase the patch without quoting the matched text.".to_owned(),
                ));
            }
        }
    }
    match assess_patch_safety(
        &action,
        step_context.settings.approval_policy(),
        turn_environment.permission_profile(),
        matching,
    )
    .map_err(|error| {
        FunctionCallError::RespondToModel(format!("failed to check patch permissions: {error}"))
    })? {
        SafetyCheck::AutoApprove => Ok(ApplyPatchRuntimeInvocation {
            action,
            auto_approved: true,
            exec_approval_requirement: ExecApprovalRequirement::Skip {
                bypass_sandbox: false,
                proposed_execpolicy_amendment: None,
            },
        }),
        SafetyCheck::AskUser => {
            // Delegate the approval prompt (including cached approvals) to the
            // tool runtime, consistent with how shell/unified_exec approvals
            // are orchestrator-driven.
            Ok(ApplyPatchRuntimeInvocation {
                action,
                auto_approved: false,
                exec_approval_requirement: ExecApprovalRequirement::NeedsApproval {
                    reason: None,
                    proposed_execpolicy_amendment: None,
                },
            })
        }
        SafetyCheck::Reject { reason } => Err(FunctionCallError::RespondToModel(format!(
            "patch rejected: {reason}"
        ))),
    }
}

pub(crate) fn convert_apply_patch_to_protocol(
    action: &ApplyPatchAction,
) -> HashMap<PathBuf, FileChange> {
    let mut result = HashMap::with_capacity(action.changes().len());
    for (path, change) in action.changes() {
        let protocol_change = match change {
            ApplyPatchFileChange::Add { content, .. } => FileChange::Add {
                content: content.clone(),
            },
            ApplyPatchFileChange::Delete { content } => FileChange::Delete {
                content: content.clone(),
            },
            ApplyPatchFileChange::Update {
                unified_diff,
                move_path,
                new_content: _new_content,
            } => FileChange::Update {
                unified_diff: unified_diff.clone(),
                move_path: move_path.as_ref().map(PathUri::to_path_buf),
            },
        };
        // TODO(anp): Carry PathUri through patch protocol events once app-server and rollout
        // compatibility no longer require path-flavored strings.
        result.insert(path.to_path_buf(), protocol_change);
    }
    result
}

#[cfg(test)]
#[path = "apply_patch_tests.rs"]
mod tests;
