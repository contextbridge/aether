fn main() {
    let schema = schemars::schema_for!(aether_project::AetherSettings);
    println!("{}", serde_json::to_string_pretty(&schema).expect("AetherSettings schema serializes to JSON"));
}
