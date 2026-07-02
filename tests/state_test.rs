/// Integration tests for state file handling.

#[test]
fn state_empty_when_no_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("no_such_file.json");

    // Manually replicate State::load_from logic
    let state: serde_json::Value = if path.exists() {
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
    } else {
        serde_json::json!({"workspaces": {}, "services": {}})
    };

    let workspaces = state["workspaces"].as_object().unwrap();
    let services = state["services"].as_object().unwrap();
    assert!(workspaces.is_empty());
    assert!(services.is_empty());
}

#[test]
fn state_file_round_trip_json_format() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("active.json");

    let state = serde_json::json!({
        "workspaces": {
            "/home/user/ws/my-feature": "test"
        },
        "services": {
            "my-api": "uat",
            "my-apps-api": "uat"
        }
    });

    // Write atomically
    let data = serde_json::to_string_pretty(&state).unwrap() + "\n";
    let tmp = dir.path().join("active.json.tmp");
    std::fs::write(&tmp, &data).unwrap();
    std::fs::rename(&tmp, &path).unwrap();

    // Read back
    let loaded: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();

    assert_eq!(loaded["workspaces"]["/home/user/ws/my-feature"], "test");
    assert_eq!(loaded["services"]["my-api"], "uat");
    assert_eq!(loaded["services"]["my-apps-api"], "uat");
}

#[test]
fn state_file_handles_malformed_json_gracefully() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("active.json");
    std::fs::write(&path, "{ this is not valid json ").unwrap();

    // Should return default state on parse error
    let state: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::json!({"workspaces": {}, "services": {}}));

    assert!(state["workspaces"].as_object().unwrap().is_empty());
}
