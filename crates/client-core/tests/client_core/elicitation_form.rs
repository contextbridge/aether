use agent_client_protocol::schema::v2::{
    BooleanPropertySchema, ElicitationContentValue as Value, ElicitationPropertySchema, ElicitationSchema, EnumOption,
    IntegerPropertySchema, MultiSelectPropertySchema, NumberPropertySchema, StringPropertySchema,
};
use client_core::elicitation::{
    ElicitationAnswer, FieldError, FieldInput, FormError, FormField, FormFieldKind, SelectOption, accept_content,
    form_fields,
};
use serde_json::json;

#[test]
fn fields_take_their_label_description_and_control_from_the_schema() {
    let schema = ElicitationSchema::new()
        .property("count", IntegerPropertySchema::new().title("Count").description("How many"), true)
        .property("enabled", BooleanPropertySchema::new(), false)
        .property("labels", MultiSelectPropertySchema::new(vec!["a".into()]), false)
        .property("name", StringPropertySchema::new(), false)
        .property("ratio", NumberPropertySchema::new(), false)
        .property("size", StringPropertySchema::new().one_of(vec![EnumOption::new("s", "Small")]), false);

    let fields = fields(&schema);

    assert_eq!(
        (fields[0].label.as_str(), fields[0].description.as_deref(), fields[0].required),
        ("Count", Some("How many"), true)
    );
    assert_eq!(
        (fields[1].label.as_str(), fields[1].description.as_deref(), fields[1].required),
        ("enabled", None, false)
    );
    let kinds: Vec<_> = fields.iter().map(|field| field.kind.clone()).collect();
    assert_eq!(
        kinds,
        [
            FormFieldKind::Integer,
            FormFieldKind::Boolean,
            FormFieldKind::Multi(vec![option("a", "a")]),
            FormFieldKind::Text,
            FormFieldKind::Number,
            FormFieldKind::Single(vec![option("s", "Small")]),
        ]
    );
}

#[test]
fn defaults_submit_as_typed_values() {
    let colors = || StringPropertySchema::new().enum_values(vec!["red".into(), "green".into()]);
    let cases: [(ElicitationSchema, serde_json::Value); 8] = [
        (ElicitationSchema::new(), json!({})),
        (optional("greeting", StringPropertySchema::new().default_value("hello")), json!({"greeting": "hello"})),
        (optional("name", StringPropertySchema::new()), json!({})),
        (optional("count", IntegerPropertySchema::new().default_value(42)), json!({"count": 42})),
        (optional("score", NumberPropertySchema::new().default_value(2.5)), json!({"score": 2.5})),
        (optional("approved", BooleanPropertySchema::new().default_value(true)), json!({"approved": true})),
        (required("color", colors()), json!({"color": "red"})),
        (required("color", colors().default_value("blue")), json!({"color": "red"})),
    ];
    for (schema, expected) in cases {
        assert_eq!(submitted_defaults(&schema), expected);
    }
    let tags = MultiSelectPropertySchema::new(vec!["fast".into(), "cheap".into()]).default_value(vec!["cheap".into()]);
    assert_eq!(submitted_defaults(&required("tags", tags)), json!({"tags": ["cheap"]}));
    assert_eq!(submitted_defaults(&required("color", colors().default_value("green"))), json!({"color": "green"}));
}

#[test]
fn input_is_validated_against_its_field() {
    let not_a_number = || Err(FieldError::NotANumber { label: "score".into() });
    let cases: Vec<(ElicitationSchema, FieldInput, Result<Option<Value>, FieldError>)> = vec![
        (required("name", StringPropertySchema::new()), text(""), Err(FieldError::Required { label: "name".into() })),
        (optional("name", StringPropertySchema::new()), text(""), Ok(None)),
        (optional("count", IntegerPropertySchema::new()), text(""), Ok(None)),
        (
            optional("count", IntegerPropertySchema::new()),
            text("1.5"),
            Err(FieldError::NotAnInteger { label: "count".into() }),
        ),
        (optional("count", IntegerPropertySchema::new()), text("15"), Ok(Some(Value::Integer(15)))),
        (optional("score", NumberPropertySchema::new()), text(""), Ok(None)),
        (optional("score", NumberPropertySchema::new()), text("4a"), not_a_number()),
        (optional("score", NumberPropertySchema::new()), text("inf"), not_a_number()),
        (optional("score", NumberPropertySchema::new()), text("NaN"), not_a_number()),
        (optional("score", NumberPropertySchema::new()), text("4"), Ok(Some(Value::Number(4.0)))),
        (
            required("color", StringPropertySchema::new().enum_values(vec!["red".into()])),
            FieldInput::Single(1),
            Err(FieldError::Required { label: "color".into() }),
        ),
        (
            required("tags", MultiSelectPropertySchema::new(vec!["fast".into()])),
            FieldInput::Multi(vec![false]),
            Ok(Some(Value::StringArray(vec![]))),
        ),
        (optional("notify", BooleanPropertySchema::new()), text("yes"), Ok(None)),
        (
            required("notify", BooleanPropertySchema::new()),
            text("yes"),
            Err(FieldError::Required { label: "notify".into() }),
        ),
    ];
    for (schema, input, expected) in cases {
        assert_eq!(fields(&schema).remove(0).value(&input), expected, "{input:?}");
    }
}

#[test]
fn accept_content_names_the_first_field_standing_in_the_way() {
    let schema = ElicitationSchema::new()
        .property("first", StringPropertySchema::new(), false)
        .property("second", IntegerPropertySchema::new(), true)
        .property("third", StringPropertySchema::new().title("Third"), true);
    let fields = fields(&schema);

    let error = accept_content(fields.iter().zip([text("ok"), text("x"), text("")])).unwrap_err();
    assert_eq!(error, FormError { index: 1, error: FieldError::NotAnInteger { label: "second".into() } });
    assert_eq!(error.to_string(), "second must be an integer");

    let error = accept_content(fields.iter().zip([text("ok"), text("1"), text("")])).unwrap_err();
    assert_eq!(error.to_string(), "Third is required");
}

#[test]
fn display_values_read_back_what_the_user_chose() {
    let urgency =
        StringPropertySchema::new().one_of(vec![EnumOption::new("low", "Low"), EnumOption::new("high", "High")]);
    let schema = ElicitationSchema::new()
        .property("count", IntegerPropertySchema::new(), false)
        .property("notify", BooleanPropertySchema::new(), false)
        .property("tags", MultiSelectPropertySchema::new(vec!["fast".into(), "reliable".into(), "cheap".into()]), false)
        .property("urgency", urgency, false);
    let fields = fields(&schema);
    let inputs =
        [text("12"), FieldInput::Boolean(true), FieldInput::Multi(vec![true, false, true]), FieldInput::Single(1)];

    let shown: Vec<_> = fields.iter().zip(&inputs).map(|(field, input)| field.display_value(input)).collect();

    assert_eq!(shown, ["12", "Yes", "fast, cheap", "High"]);
    assert_eq!(fields[1].display_value(&FieldInput::Boolean(false)), "No");
    assert_eq!(fields[3].display_value(&FieldInput::Single(5)), "", "nothing chosen reads back empty");
}

fn fields(schema: &ElicitationSchema) -> Vec<FormField> {
    form_fields(schema).expect("every property is supported")
}

fn submitted_defaults(schema: &ElicitationSchema) -> serde_json::Value {
    let fields = fields(schema);
    let Ok(ElicitationAnswer::Accept(Some(content))) =
        accept_content(fields.iter().map(|field| (field, field.default_input.clone())))
    else {
        panic!("defaults are a valid answer");
    };
    serde_json::to_value(content).expect("elicitation content serializes")
}

fn required(name: &str, property: impl Into<ElicitationPropertySchema>) -> ElicitationSchema {
    ElicitationSchema::new().property(name, property, true)
}

fn optional(name: &str, property: impl Into<ElicitationPropertySchema>) -> ElicitationSchema {
    ElicitationSchema::new().property(name, property, false)
}

fn text(value: &str) -> FieldInput {
    FieldInput::Text(value.to_string())
}

fn option(value: &str, title: &str) -> SelectOption {
    SelectOption { value: value.to_string(), title: title.to_string() }
}
