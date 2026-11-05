//! Kernel golden runner (architecture §5): every scenario that is not
//! shell-level runs through the five stages and lands in `goldens/kernel/`
//! as a reviewed insta snapshot. Regenerate with `INSTA_UPDATE=always`.
#![allow(clippy::unwrap_used, clippy::panic, reason = "test code")]

mod support;

use support::*;

fn goldens_dir() -> std::path::PathBuf {
    fixtures_dir().join("goldens/kernel")
}

fn snapshot_value(value: &serde_json::Value) -> serde_yaml::Value {
    // JSON's arbitrary_precision feature is unified across the workspace. Its
    // Number serializer exposes a private map to non-JSON serializers (including
    // insta), so normalize through JSON text before producing a YAML snapshot.
    serde_yaml::from_str(&serde_json::to_string(value).unwrap()).unwrap()
}

#[test]
fn snapshot_numbers_are_plain_yaml_scalars() {
    let json = serde_json::json!({"nested": [1, -2, 1.25, u64::MAX], "zero": 0});
    let expected: serde_yaml::Value =
        serde_yaml::from_str("nested: [1, -2, 1.25, 18446744073709551615]\nzero: 0").unwrap();
    assert_eq!(snapshot_value(&json), expected);
}

#[test]
fn kernel_goldens() {
    let mut count = 0;
    for scenario in load_all_scenarios() {
        if scenario.markers.iter().any(|m| m == "S") || !scenario_selected(&scenario.id) {
            continue;
        }
        count += 1;
        let pipeline = Pipeline::from_scenario(&scenario);
        let body = capture_body(&pipeline, &all_windows(&scenario));
        let output = serde_json::json!({
            "scenario": scenario.id,
            "as_of": scenario.policy.as_of.to_string(),
            "base_currency": scenario.policy.base_currency,
            "baseline": body,
        });
        insta::with_settings!({
            snapshot_path => goldens_dir(),
            prepend_module_to_snapshot => false,
            omit_expression => true,
            // Key order must not depend on serde_json's `preserve_order`,
            // which a dependency may enable for the whole build.
            sort_maps => true,
        }, {
            insta::assert_yaml_snapshot!(scenario.id.clone(), snapshot_value(&output));
        });
    }
    assert!(count > 0, "no scenarios matched");
}
