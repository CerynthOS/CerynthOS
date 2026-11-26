use cerynth_agent_api::{Operation, OperationStatus, Request, Session};
use serde_json::Value;

#[test]
fn actual_previous_checkpoint_requests_and_unknown_outcomes_remain_readable() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/legacy-2d8b5d4.json")).unwrap();
    assert_eq!(fixture["producer_revision"], "2d8b5d4");
    for value in fixture["requests"].as_array().unwrap() {
        let request: Request = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&request).unwrap(), *value);
        assert_eq!(request.version, 1);
        assert!(request.expected_contract.is_none());
        if let Operation::Create { spec } = request.operation {
            assert!(spec.validate().is_ok());
            assert!(spec.runtime.guard.unwrap().remotes.is_empty());
        }
    }
    let session: Session = serde_json::from_value(fixture["session"].clone()).unwrap();
    assert_eq!(serde_json::to_value(&session).unwrap(), fixture["session"]);
    assert!(session.execution.resource_allocation.is_none());
    assert!(session.execution.model_residency.is_none());
    assert_eq!(
        session.execution.operations[0].status,
        OperationStatus::Unknown
    );
    assert_eq!(
        session.execution.operations[0].fingerprint,
        "legacy-public-action"
    );
}

#[test]
fn candidate_contract_is_deterministic_and_does_not_invent_team_approval() {
    let first = cerynth_agent_api::contract::manifest();
    let second = cerynth_agent_api::contract::manifest();
    assert_eq!(first, second);
    let digest = first["contract_id"].as_str().unwrap();
    assert_eq!(digest.len(), 64);
    assert!(digest.bytes().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(first["team_approval_claimed"], false);
    assert_eq!(first["features"]["tool_effect_deduplication"], 1);
}
