use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, Time};
use k8s_openapi::jiff::Timestamp;
use kube::api::ObjectMeta;

pub(crate) fn set_condition(
    mut conditions: Vec<Condition>,
    metadata: ObjectMeta,
    condition_type: &str,
    condition_status: String,
    condition_reason: String,
    condition_message: String,
) -> Vec<Condition> {
    if let Some(condition) = conditions.iter_mut().find(|c| c.type_ == condition_type) {
        condition.status = condition_status;
        condition.last_transition_time = Time::from(Timestamp::now());
        condition.message = condition_message;
        condition.reason = condition_reason;
        condition.observed_generation = metadata.generation;
    } else {
        conditions.push(Condition {
            last_transition_time: Time::from(Timestamp::now()),
            message: condition_message,
            observed_generation: metadata.generation,
            reason: condition_reason,
            status: condition_status,
            type_: condition_type.to_string(),
        });
    }
    conditions
}

pub(crate) fn init_conditions(types: Vec<String>) -> Vec<Condition> {
    let mut conditions: Vec<Condition> = vec![];
    types.iter().for_each(|t| {
        let condition = Condition {
            last_transition_time: Time::from(Timestamp::now()),
            message: "Reconciliation started".to_string(),
            observed_generation: Some(1),
            reason: "Reconciling".to_string(),
            status: "Unknown".to_string(),
            type_: t.clone(),
        };
        conditions.push(condition);
    });
    conditions
}

pub(crate) fn is_condition_true(mut conditions: Vec<Condition>, condition_type: &str) -> bool {
    if let Some(condition) = conditions.iter_mut().find(|c| c.type_ == condition_type) {
        return condition.status == "True";
    }
    false
}

pub(crate) fn is_condition_false(mut conditions: Vec<Condition>, condition_type: &str) -> bool {
    if let Some(condition) = conditions.iter_mut().find(|c| c.type_ == condition_type) {
        return condition.status == "False";
    }
    false
}

pub(crate) fn is_condition_unknown(mut conditions: Vec<Condition>, condition_type: &str) -> bool {
    if let Some(condition) = conditions.iter_mut().find(|c| c.type_ == condition_type) {
        return condition.status == "Unknown";
    }
    false
}
