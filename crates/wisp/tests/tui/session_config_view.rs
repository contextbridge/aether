use acp_utils::config_meta::SelectOptionMeta;
use acp_utils::config_option_id::ConfigOptionId;
use agent_client_protocol::schema::v1::{SessionConfigOption, SessionConfigSelectGroup, SessionConfigSelectOption};
use utils::ReasoningEffort;
use wisp::session::session_config_view::{LocalConfigOption, LocalConfigView};

fn option(value: &str, name: &str) -> SessionConfigSelectOption {
    SessionConfigSelectOption::new(value.to_string(), name.to_string())
}

#[test]
fn projects_grouped_choices_current_display_and_model_metadata() {
    let image = option("image", "Image")
        .meta(SelectOptionMeta { reasoning_levels: vec![], supports_image: true, supports_audio: false }.into_meta());
    let audio = option("audio", "Audio")
        .meta(SelectOptionMeta { reasoning_levels: vec![], supports_image: false, supports_audio: true }.into_meta());
    let mut model =
        SessionConfigOption::select("model", "Model", "image,audio", Vec::<SessionConfigSelectOption>::new());
    if let agent_client_protocol::schema::v1::SessionConfigKind::Select(select) = &mut model.kind {
        select.options = agent_client_protocol::schema::v1::SessionConfigSelectOptions::Grouped(vec![
            SessionConfigSelectGroup::new("media", "Media", vec![image, audio]),
        ]);
    }
    let reasoning = SessionConfigOption::select(
        "reasoning_effort",
        "Reasoning",
        "high",
        vec![option("low", "Low"), option("high", "High")],
    );
    let options: Vec<LocalConfigOption> = [model, reasoning].into_iter().map(LocalConfigOption::from_acp).collect();
    let view = LocalConfigView::new(&options);

    assert_eq!(view.current_values(ConfigOptionId::Model), ["image", "audio"]);
    assert_eq!(view.current_display_name(ConfigOptionId::Model).as_deref(), Some("Image + Audio"));
    let model_options = view.flattened_options(ConfigOptionId::Model);
    assert_eq!(model_options.len(), 2);
    assert!(model_options.iter().all(|option| option.group.as_deref() == Some("Media")));
    assert_eq!(
        view.selected_model_metadata(),
        vec![
            SelectOptionMeta { reasoning_levels: vec![], supports_image: true, supports_audio: false },
            SelectOptionMeta { reasoning_levels: vec![], supports_image: false, supports_audio: true },
        ]
    );
    assert_eq!(view.reasoning_levels(), vec![ReasoningEffort::Low, ReasoningEffort::High]);
    assert_eq!(view.reasoning_effort(), Some(ReasoningEffort::High));
}
