#![cfg(feature = "cli")]
use serde_json::Value;
use std::process::Command;
#[test]
fn executable_discovery_is_json_only_and_needs_no_config_or_credentials() {
    let binary = env!("CARGO_BIN_EXE_tg-agent");
    for args in [
        &["tools"][..],
        &["schema"][..],
        &["schema", "request"][..],
        &["schema", "response"][..],
    ] {
        let output = Command::new(binary)
            .args(args)
            .env_remove("TELEGRAM_BOT_TOKEN")
            .env_remove("TG_BOT_TOKEN")
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(value.is_object());
        if args == ["tools"] {
            assert_eq!(value["tools"].as_array().unwrap().len(), 8);
        }
    }
    let output = Command::new(binary)
        .args(["schema", "--config", "PRIVATE_PATH"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["code"], "agent_usage");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("PRIVATE"));
    assert!(output.stderr.is_empty());
}

#[cfg(not(feature = "mtproto"))]
#[test]
fn extended_discovery_requires_the_explicit_mtproto_build_feature() {
    let output = Command::new(env!("CARGO_BIN_EXE_tg-agent"))
        .args(["tools", "--mtproto"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap().trim(),
        "agent_mtproto_feature_required"
    );
}
