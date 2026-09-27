use schemars::JsonSchema;
use schemars::generate::{SchemaGenerator, SchemaSettings};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub struct SchemaDocument {
    outputs: SchemaGenerator,
    inputs: SchemaGenerator,
    external: BTreeSet<String>,
    roots: Vec<String>,
}

impl Default for SchemaDocument {
    fn default() -> Self {
        Self {
            outputs: SchemaSettings::default().for_serialize().into_generator(),
            inputs: SchemaSettings::default().for_deserialize().into_generator(),
            external: BTreeSet::new(),
            roots: Vec::new(),
        }
    }
}

impl SchemaDocument {
    pub fn with_external(register: impl Fn(&mut SchemaGenerator)) -> Self {
        let mut document = Self::default();
        register(&mut document.outputs);
        register(&mut document.inputs);
        document.external =
            document.outputs.definitions().keys().chain(document.inputs.definitions().keys()).cloned().collect();
        document
    }

    pub fn output<T: JsonSchema>(mut self) -> Self {
        let root = root::<T>(&mut self.outputs);
        self.roots.push(root);
        self
    }

    pub fn input<T: JsonSchema>(mut self) -> Self {
        let root = root::<T>(&mut self.inputs);
        self.roots.push(root);
        self
    }

    pub fn build(mut self) -> Value {
        let mut definitions = self.outputs.take_definitions(true);
        for (name, schema) in self.inputs.take_definitions(true) {
            if let Some(output) = definitions.get(&name).filter(|_| !self.external.contains(&name)) {
                assert_eq!(
                    output, &schema,
                    "`{name}` is written and read with different schemas, so it cannot be declared once"
                );
            }
            definitions.insert(name, schema);
        }

        for name in definitions.keys().filter(|name| !self.external.contains(*name)) {
            let base = name.trim_end_matches(|character: char| character.is_ascii_digit());
            assert!(
                base == name || !self.external.contains(base),
                "`{base}` names both an external type and one of ours, which schemars renamed to `{name}`; \
                 give ours a distinct #[schemars(rename)]"
            );
        }

        json!({ "roots": self.roots, "$defs": definitions, "external": self.external })
    }

    pub fn print(self) {
        println!("{}", serde_json::to_string_pretty(&self.build()).expect("schema document serializes to JSON"));
    }
}

fn root<T: JsonSchema>(generator: &mut SchemaGenerator) -> String {
    let schema = generator.subschema_for::<T>();
    let reference = schema.get("$ref").and_then(|reference| reference.as_str());
    reference
        .and_then(|reference| reference.strip_prefix("#/$defs/"))
        .expect("a root type has its own definition")
        .to_owned()
}
