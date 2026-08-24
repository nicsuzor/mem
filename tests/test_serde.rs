#[test]
fn test_serde_index() {
    let mut v = serde_json::json!({ "id": "1" });
    v["angle"] = serde_json::json!(1.5);
    println!("{}", v);
}
