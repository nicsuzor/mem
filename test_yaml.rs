use serde_json::json;

fn main() {
    let mut fm = serde_json::Map::new();
    fm.insert("title".to_string(), json!("Hello"));
    let yaml = serde_yaml::to_string(&fm).unwrap();
    println!("YAML output:\n{:?}", yaml);
}
