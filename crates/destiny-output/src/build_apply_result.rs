//! Typed projection of an executed build apply: per-step outcomes, overall result and the
//! existing action exit codes (no new ones).
use crate::builds::{category_exit, clean_text as clean, step_json, step_text, summary_json};
use destiny_app::build_apply_exec::{
    ApplyLocalState, ApplyStatus, BuildApplyExecution, ItemStatus, SkipCause, StepFailure,
};
use serde_json::{Value, json};

fn outcome_json(
    name: Option<String>,
    item: &destiny_app::ItemInstanceId,
    status: &ItemStatus,
) -> Value {
    let mut value = json!({"item":item.as_str(), "name":name});
    let o = value.as_object_mut().expect("object");
    match status {
        ItemStatus::Succeeded => {
            o.insert("status".into(), json!("succeeded"));
        }
        ItemStatus::Skipped(cause) => {
            o.insert("status".into(), json!("skipped"));
            match cause {
                SkipCause::DependencyFailed(step) => {
                    o.insert("cause".into(), json!("dependency_failed"));
                    o.insert("depends_on_step".into(), json!(step + 1));
                }
                SkipCause::ProviderStopped => {
                    o.insert("cause".into(), json!("provider_stopped"));
                }
                SkipCause::PlanRefused => {
                    o.insert("cause".into(), json!("plan_refused"));
                }
            }
        }
        ItemStatus::Failed(failure) => {
            o.insert("status".into(), json!("failed"));
            let (kind, reason, code) = match failure {
                StepFailure::Rejected(reason) => ("rejected", Some(*reason), None),
                StepFailure::BungieCode(code) => ("bungie_code", None, Some(*code)),
                StepFailure::NotReported => ("not_reported", None, None),
                StepFailure::AuthenticationRefused => ("authentication_refused", None, None),
                StepFailure::Transient => ("transient", None, None),
                StepFailure::Uncertain => ("uncertain", None, None),
                StepFailure::NotDispatched(category) => {
                    ("not_dispatched", Some(category_exit(*category).0), None)
                }
            };
            o.insert("failure".into(), json!(kind));
            if let Some(reason) = reason {
                o.insert("reason".into(), json!(reason));
                if let Some(message) = crate::rejection_message(reason) {
                    o.insert("message".into(), json!(message));
                }
            }
            if let Some(code) = code {
                o.insert("code".into(), json!(code));
            }
        }
    }
    value
}

/// The JSON value and the process exit code for one executed apply.
pub fn execution(applied: &BuildApplyExecution) -> (Value, i32) {
    let preview = &applied.preview;
    let run = &applied.execution;
    let plan = &preview.plan;
    let name = |id: &destiny_app::ItemInstanceId| {
        preview
            .names
            .iter()
            .find(|(i, _)| i == id)
            .map(|(_, n)| n.clone())
    };
    let steps: Vec<_> = run
        .steps
        .iter()
        .map(|report| {
            let mut step = step_json(&report.step, &name);
            step["ordinal"] = json!(report.index + 1);
            step["needs"] = json!(report.step.needs.iter().map(|n| n + 1).collect::<Vec<_>>());
            step["outcomes"] = json!(
                report
                    .outcomes
                    .iter()
                    .map(|o| outcome_json(name(&o.item), &o.item, &o.status))
                    .collect::<Vec<_>>()
            );
            step
        })
        .collect();
    let all = run.steps.iter().flat_map(|s| &s.outcomes);
    let count = |keep: fn(&ItemStatus) -> bool| all.clone().filter(|o| keep(&o.status)).count();
    let counts = json!({
        "succeeded":count(|s| matches!(s, ItemStatus::Succeeded)),
        "failed":count(|s| matches!(s, ItemStatus::Failed(_))),
        "skipped":count(|s| matches!(s, ItemStatus::Skipped(_))),
    });
    // Build items that could not even be planned mean the build is not fully applied.
    let unplanned = !plan.skipped.is_empty();
    let (result, mut exit_code) = match run.status {
        ApplyStatus::Applied | ApplyStatus::NoOp if unplanned => ("partial", 41),
        ApplyStatus::Applied => ("applied", 0),
        ApplyStatus::NoOp => ("no_op", 0),
        ApplyStatus::Partial => ("partial", 41),
        ApplyStatus::Failed => ("failed", 41),
        ApplyStatus::Refused(category) => ("refused", category_exit(category).1),
    };
    let (local_state, reason) = match run.local_state {
        ApplyLocalState::Unchanged => ("unchanged", None),
        ApplyLocalState::Updated => ("updated", None),
        ApplyLocalState::RefreshPending => ("refresh_pending", None),
        ApplyLocalState::Failed => (
            "failed",
            Some("local state could not be updated after the action"),
        ),
    };
    if run.local_state == ApplyLocalState::Failed {
        exit_code = 42;
    }
    let value = json!({"kind":"build_apply", "dry_run":false, "result":result,
        "summary":summary_json(&preview.summary), "target_character":plan.target.as_str(),
        "dispatched":run.dispatched, "steps":steps, "counts":counts,
        "skipped":plan.skipped.iter().map(|s| json!({"spec_index":s.spec_index, "reason":s.reason.as_str()})).collect::<Vec<_>>(),
        "already_there":plan.already_there,
        "clear_skipped":plan.clear_skipped.iter().map(|(i, r)| json!({"item":i.as_str(), "reason":r.as_str()})).collect::<Vec<_>>(),
        "not_applied":plan.not_applied.iter().map(|r| json!({"outcome":"not_applied", "reason":r})).collect::<Vec<_>>(),
        "unknown":plan.unknown, "local_state":local_state, "reason":reason,
        "metadata":{"checked_at":preview.metadata.sync.checked_at.seconds(),
            "revision":preview.metadata.sync.revision.value()}});
    (value, exit_code)
}

pub fn render(v: &Value) -> String {
    let c = &v["counts"];
    let mut lines = vec![
        format!(
            "Applied {} on character {}: {} ({} succeeded, {} failed, {} skipped)",
            clean(v["summary"]["name"].as_str().unwrap_or("")),
            clean(v["target_character"].as_str().unwrap_or("")),
            v["result"].as_str().unwrap_or(""),
            c["succeeded"],
            c["failed"],
            c["skipped"]
        ),
        "Bungie acknowledgements are not re-read; do not blindly retry.".into(),
    ];
    for step in v["steps"].as_array().into_iter().flatten() {
        let text = step_text(step);
        let outcomes = step["outcomes"].as_array().cloned().unwrap_or_default();
        let status = if outcomes.iter().all(|o| o["status"] == "succeeded") {
            "succeeded".to_string()
        } else {
            let o = outcomes
                .iter()
                .find(|o| o["status"] != "succeeded")
                .unwrap_or(&Value::Null);
            let detail = match o["status"].as_str() {
                Some("skipped") => match o["cause"].as_str() {
                    Some("dependency_failed") => {
                        format!("step {} did not succeed", o["depends_on_step"])
                    }
                    Some("provider_stopped") => "Bungie refused earlier requests".into(),
                    _ => "plan refused".into(),
                },
                _ => o["reason"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| o["code"].as_i64().map(|c| format!("Bungie code {c}")))
                    .or_else(|| o["failure"].as_str().map(str::to_owned))
                    .unwrap_or_default(),
            };
            format!("{} ({detail})", o["status"].as_str().unwrap_or(""))
        };
        lines.push(format!(
            "{}. [{}] {status}: {text}",
            step["ordinal"],
            step["phase"].as_str().unwrap_or("")
        ));
    }
    for skip in v["skipped"].as_array().into_iter().flatten() {
        lines.push(format!(
            "Skipped item {}: {}",
            skip["spec_index"], skip["reason"]
        ));
    }
    for skip in v["clear_skipped"].as_array().into_iter().flatten() {
        lines.push(format!("Not cleared {}: {}", skip["item"], skip["reason"]));
    }
    if !v["not_applied"].as_array().is_none_or(Vec::is_empty) {
        lines.push("Not applied: sockets, mods and subclass (BE-007b)".into());
    }
    if let Some(reason) = v["reason"].as_str() {
        lines.push(clean(reason));
    }
    lines.push(format!(
        "Local state: {}",
        v["local_state"].as_str().unwrap_or("").replace('_', " ")
    ));
    lines.join("\n")
}
