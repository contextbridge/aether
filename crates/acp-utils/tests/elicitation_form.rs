use acp_utils::elicitation::{
    FieldError, FieldInput, FormError, FormField, FormFieldKind, SelectOption, accept_content, form_fields,
};
use agent_client_protocol::schema::v2::{
    BooleanPropertySchema, ElicitationContentValue, ElicitationPropertySchema, ElicitationSchema, EnumOption,
    IntegerPropertySchema, MultiSelectPropertySchema, NumberPropertySchema, StringPropertySchema,
};
use serde_json::{Value, json};

#[test]
fn fields_take_their_label_and_description_from_the_schema() {
    let schema = ElicitationSchema::new()
        .property("name", StringPropertySchema::new().title("Your Name").description("Enter your full name"), true)
        .property("nickname", StringPropertySchema::new(), false);

    let fields = fields(&schema);

    assert_eq!(fields[0].label(), "Your Name");
    assert_eq!(fields[0].description(), Some("Enter your full name"));
    assert!(fields[0].required());
    assert_eq!(fields[1].label(), "nickname", "an untitled field is labelled by its name");
    assert_eq!(fields[1].description(), None);
    assert!(!fields[1].required());
}

#[test]
fn each_property_kind_becomes_the_matching_control() {
    let schema = ElicitationSchema::new()
        .property("count", IntegerPropertySchema::new(), false)
        .property("enabled", BooleanPropertySchema::new(), false)
        .property("labels", MultiSelectPropertySchema::new(vec!["a".into(), "b".into()]), false)
        .property("name", StringPropertySchema::new(), false)
        .property("ratio", NumberPropertySchema::new(), false)
        .property(
            "size",
            StringPropertySchema::new().one_of(vec![EnumOption::new("s", "Small"), EnumOption::new("l", "Large")]),
            false,
        );

    let kinds = fields(&schema).into_iter().map(|field| field.kind().clone()).collect::<Vec<_>>();

    assert_eq!(
        kinds,
        vec![
            FormFieldKind::Integer,
            FormFieldKind::Boolean,
            FormFieldKind::Multi(vec![option("a", "a"), option("b", "b")]),
            FormFieldKind::Text,
            FormFieldKind::Number,
            FormFieldKind::Single(vec![option("s", "Small"), option("l", "Large")]),
        ]
    );
}

#[test]
fn text_field_default_submits_as_a_string() {
    let schema = optional("greeting", StringPropertySchema::new().default_value("hello"));
    assert_eq!(submitted_defaults(&schema), json!({"greeting": "hello"}));
}

#[test]
fn numeric_defaults_submit_as_parsed_numbers() {
    let schema = ElicitationSchema::new()
        .property("count", IntegerPropertySchema::new().minimum(0).maximum(100).default_value(42), false)
        .property("score", NumberPropertySchema::new().minimum(0.0).maximum(100.0).default_value(2.5), false);
    assert_eq!(submitted_defaults(&schema), json!({"count": 42, "score": 2.5}));
}

#[test]
fn defaults_submit_as_typed_values_and_empty_text_is_omitted() {
    let schema = ElicitationSchema::new()
        .property("name", StringPropertySchema::new().title("Name"), false)
        .property("approved", BooleanPropertySchema::new().default_value(true), false)
        .property(
            "color",
            StringPropertySchema::new().enum_values(vec!["red".into(), "green".into()]).default_value("green"),
            false,
        )
        .property("size", StringPropertySchema::new().enum_values(vec!["s".into(), "m".into()]), true);
    assert_eq!(submitted_defaults(&schema), json!({"approved": true, "color": "green", "size": "s"}));
}

#[test]
fn a_single_select_whose_default_is_not_offered_starts_on_its_first_option() {
    let schema = required(
        "color",
        StringPropertySchema::new().enum_values(vec!["red".into(), "green".into()]).default_value("blue"),
    );
    assert_eq!(fields(&schema)[0].default_input(), &FieldInput::Single(0));
}

#[test]
fn empty_form_submits_an_empty_object() {
    assert_eq!(submitted_defaults(&ElicitationSchema::new()), json!({}));
}

#[test]
fn multi_select_defaults_submit_as_the_selected_array() {
    let schema = required(
        "tags",
        MultiSelectPropertySchema::new(vec!["fast".into(), "reliable".into(), "cheap".into()])
            .default_value(vec!["reliable".to_string()]),
    );
    assert_eq!(submitted_defaults(&schema), json!({"tags": ["reliable"]}));
}

#[test]
fn permission_like_form_submits_its_default_deny() {
    let schema = required(
        "decision",
        StringPropertySchema::new().enum_values(vec!["allow".into(), "deny".into()]).default_value("deny"),
    );
    assert_eq!(submitted_defaults(&schema), json!({"decision": "deny"}));
}

#[test]
fn a_required_field_left_empty_is_rejected_by_its_label() {
    let field = only_field(&required("name", StringPropertySchema::new().title("Name")));

    let error = field.value(&text("")).unwrap_err();

    assert_eq!(error, FieldError::Required { label: "Name".into() });
    assert_eq!(error.to_string(), "Name is required");
}

#[test]
fn an_optional_field_left_empty_sends_nothing() {
    for property in [
        ElicitationPropertySchema::from(StringPropertySchema::new()),
        IntegerPropertySchema::new().into(),
        NumberPropertySchema::new().into(),
    ] {
        assert_eq!(only_field(&optional("field", property)).value(&text("")), Ok(None));
    }
}

#[test]
fn integer_field_rejects_fractional_input() {
    let field = only_field(&optional("count", IntegerPropertySchema::new()));

    assert_eq!(field.value(&text("1.5")).unwrap_err().to_string(), "count must be an integer");
    assert_eq!(field.value(&text("15")), Ok(Some(ElicitationContentValue::Integer(15))));
}

#[test]
fn number_field_rejects_non_numeric_input_and_preserves_number_type() {
    let field = only_field(&optional("score", NumberPropertySchema::new().minimum(0.0).maximum(5.0)));

    assert_eq!(field.value(&text("4a")).unwrap_err().to_string(), "score must be a number");
    assert_eq!(field.value(&text("4")), Ok(Some(ElicitationContentValue::Number(4.0))));
}

#[test]
fn number_field_rejects_values_that_are_not_finite() {
    let field = only_field(&optional("score", NumberPropertySchema::new()));
    for input in ["inf", "-inf", "NaN"] {
        assert_eq!(field.value(&text(input)), Err(FieldError::NotANumber { label: "score".into() }), "{input}");
    }
}

#[test]
fn a_required_single_select_with_nothing_chosen_is_rejected() {
    let field = only_field(&required("color", StringPropertySchema::new().enum_values(vec!["red".into()])));
    assert_eq!(field.value(&FieldInput::Single(1)), Err(FieldError::Required { label: "color".into() }));
}

#[test]
fn a_required_multi_select_accepts_no_checked_options() {
    let field = only_field(&required("tags", MultiSelectPropertySchema::new(vec!["fast".into()])));
    assert_eq!(field.value(&FieldInput::Multi(vec![false])), Ok(Some(ElicitationContentValue::StringArray(vec![]))));
}

#[test]
fn an_input_of_the_wrong_shape_counts_as_no_answer() {
    let optional_field = only_field(&optional("notify", BooleanPropertySchema::new()));
    let required_field = only_field(&required("notify", BooleanPropertySchema::new()));

    assert_eq!(optional_field.value(&text("yes")), Ok(None));
    assert_eq!(required_field.value(&text("yes")), Err(FieldError::Required { label: "notify".into() }));
}

#[test]
fn accept_content_names_the_first_field_standing_in_the_way() {
    let schema = ElicitationSchema::new()
        .property("first", StringPropertySchema::new(), false)
        .property("second", IntegerPropertySchema::new(), true)
        .property("third", StringPropertySchema::new(), true);
    let fields = fields(&schema);

    let error = accept_content(fields.iter().zip([text("ok"), text("x"), text("")])).unwrap_err();

    assert_eq!(error, FormError { index: 1, error: FieldError::NotAnInteger { label: "second".into() } });
    assert_eq!(error.to_string(), "second must be an integer");
}

#[test]
fn display_values_read_back_what_the_user_chose() {
    let schema = ElicitationSchema::new()
        .property("count", IntegerPropertySchema::new(), false)
        .property("notify", BooleanPropertySchema::new(), false)
        .property("tags", MultiSelectPropertySchema::new(vec!["fast".into(), "reliable".into(), "cheap".into()]), false)
        .property(
            "urgency",
            StringPropertySchema::new().one_of(vec![EnumOption::new("low", "Low"), EnumOption::new("high", "High")]),
            false,
        );
    let fields = fields(&schema);
    let inputs =
        [text("12"), FieldInput::Boolean(true), FieldInput::Multi(vec![true, false, true]), FieldInput::Single(1)];

    let shown = fields.iter().zip(&inputs).map(|(field, input)| field.display_value(input)).collect::<Vec<_>>();

    assert_eq!(shown, ["12", "Yes", "fast, cheap", "High"]);
    assert_eq!(fields[1].display_value(&FieldInput::Boolean(false)), "No");
    assert_eq!(fields[3].display_value(&FieldInput::Single(5)), "", "nothing chosen reads back empty");
}

fn fields(schema: &ElicitationSchema) -> Vec<FormField> {
    form_fields(schema).expect("every property is supported")
}

fn only_field(schema: &ElicitationSchema) -> FormField {
    fields(schema).remove(0)
}

fn submitted_defaults(schema: &ElicitationSchema) -> Value {
    let fields = fields(schema);
    let content = accept_content(fields.iter().map(|field| (field, field.default_input().clone())))
        .expect("defaults are a valid answer");
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
