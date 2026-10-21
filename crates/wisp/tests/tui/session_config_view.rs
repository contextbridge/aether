use super::support::{ConfigOptionId, LocalConfigOption, SelectOptionMeta, model_choice, option_group, select};
use agent_client_protocol::schema::v2::SessionConfigOption;
use client_core::session_config_view::LocalConfigView;
use utils::ReasoningEffort;

#[test]
fn control_states_survive_projection_without_default_becoming_a_capability() {
    for effort in [ReasoningEffort::Default, ReasoningEffort::Disabled] {
        let levels = [("default", "Default"), ("disabled", "Disabled"), ("low", "Low")];
        let options = [LocalConfigOption::from_acp(select("reasoning_effort", "Reasoning", effort.as_str(), &levels))];
        let view = LocalConfigView::new(&options);
        assert_eq!(view.reasoning_effort(), Some(effort));
        assert_eq!(view.reasoning_levels(), vec![ReasoningEffort::Disabled, ReasoningEffort::Low]);
    }
}

#[test]
fn projects_v2_wire_ids_and_tolerates_unknown_categories() {
    let option: SessionConfigOption = serde_json::from_value(serde_json::json!({
        "configId": "model",
        "name": "Model",
        "category": "future_category",
        "type": "select",
        "currentValue": "fast",
        "options": [{
            "groupId": "provider",
            "name": "Provider",
            "options": [{"value": "fast", "name": "Fast"}]
        }]
    }))
    .unwrap();
    let options = [LocalConfigOption::from_acp(option)];
    let view = LocalConfigView::new(&options);
    assert_eq!(options[0].id, "model");
    assert_eq!(view.current_display_name(ConfigOptionId::Model).as_deref(), Some("Fast"));
    assert_eq!(view.flattened_options(ConfigOptionId::Model)[0].group.as_deref(), Some("Provider"));
    assert_eq!(view.next_mode(), None);
}

#[test]
fn projects_grouped_choices_current_display_and_model_metadata() {
    let image = SelectOptionMeta { supports_image: true, ..Default::default() };
    let audio = SelectOptionMeta { supports_audio: true, ..Default::default() };
    let media = option_group(
        "Media",
        vec![model_choice("image", "Image", image.clone()), model_choice("audio", "Audio", audio.clone())],
    );
    let model = SessionConfigOption::select("model", "Model", "image,audio", vec![media]);
    let reasoning = select("reasoning_effort", "Reasoning", "high", &[("low", "Low"), ("high", "High")]);
    let options: Vec<LocalConfigOption> = [model, reasoning].into_iter().map(LocalConfigOption::from_acp).collect();
    let view = LocalConfigView::new(&options);

    assert_eq!(view.current_values(ConfigOptionId::Model), ["image", "audio"]);
    assert_eq!(view.current_display_name(ConfigOptionId::Model).as_deref(), Some("Image + Audio"));
    let model_options = view.flattened_options(ConfigOptionId::Model);
    assert_eq!(model_options.len(), 2);
    assert!(model_options.iter().all(|option| option.group.as_deref() == Some("Media")));
    assert_eq!(view.selected_model_metadata(), vec![image, audio]);
    assert_eq!(view.reasoning_levels(), vec![ReasoningEffort::Low, ReasoningEffort::High]);
    assert_eq!(view.reasoning_effort(), Some(ReasoningEffort::High));
}
