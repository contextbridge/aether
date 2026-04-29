fn main() {
    let schema = schemars::schema_for!(aether_project::AetherSettings);
    println!("{}", serde_json::to_string_pretty(&schema).expect("schema serialization cannot fail"));
}
