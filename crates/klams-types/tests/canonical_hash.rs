//! T017: failing `canonical_json_hash` unit test (key-order + whitespace
//! independence). T018 implements the function.

use klams_types::canonical_json_hash;

#[test]
fn key_order_does_not_affect_hash() {
    let a: serde_json::Value =
        serde_json::from_str(r#"{"host":"kubs0","ram_gb":64,"gpus":2}"#).unwrap();
    let b: serde_json::Value =
        serde_json::from_str(r#"{"gpus":2,"ram_gb":64,"host":"kubs0"}"#).unwrap();
    assert_eq!(
        canonical_json_hash("EnvFact", &a),
        canonical_json_hash("EnvFact", &b),
    );
}

#[test]
fn whitespace_does_not_affect_hash() {
    let a: serde_json::Value = serde_json::from_str(r#"{"k":1,"v":[1,2,3]}"#).unwrap();
    let b: serde_json::Value =
        serde_json::from_str("{ \"k\" : 1 ,\n  \"v\" : [ 1 , 2 , 3 ] }").unwrap();
    assert_eq!(canonical_json_hash("T", &a), canonical_json_hash("T", &b),);
}

#[test]
fn type_tag_separates_namespace() {
    let p: serde_json::Value = serde_json::from_str(r#"{"x":1}"#).unwrap();
    assert_ne!(
        canonical_json_hash("UserFact", &p),
        canonical_json_hash("TaskFact", &p),
    );
}

#[test]
fn nested_object_keys_are_sorted_too() {
    let a: serde_json::Value =
        serde_json::from_str(r#"{"outer":{"a":1,"b":{"y":2,"x":1}}}"#).unwrap();
    let b: serde_json::Value =
        serde_json::from_str(r#"{"outer":{"b":{"x":1,"y":2},"a":1}}"#).unwrap();
    assert_eq!(
        canonical_json_hash("UserFact", &a),
        canonical_json_hash("UserFact", &b),
    );
}

#[test]
fn different_payloads_hash_differently() {
    let a: serde_json::Value = serde_json::from_str(r#"{"x":1}"#).unwrap();
    let b: serde_json::Value = serde_json::from_str(r#"{"x":2}"#).unwrap();
    assert_ne!(
        canonical_json_hash("UserFact", &a),
        canonical_json_hash("UserFact", &b),
    );
}
