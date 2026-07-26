use acp_utils::config_meta::SelectOptionMeta;
use acp_utils::config_option_id::ConfigOptionId;
use agent_client_protocol::schema::{SessionConfigOption, SessionConfigSelectGroup, SessionConfigSelectOption};
use utils::ReasoningEffort;
use wisp_next::test_support::session_config_view::SessionConfigView;

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
    if let agent_client_protocol::schema::SessionConfigKind::Select(select) = &mut model.kind {
        select.options =
            agent_client_protocol::schema::SessionConfigSelectOptions::Grouped(vec![SessionConfigSelectGroup::new(
                "media",
                "Media",
                vec![image, audio],
            )]);
    }
    let reasoning = SessionConfigOption::select(
        "reasoning_effort",
        "Reasoning",
        "high",
        vec![option("low", "Low"), option("high", "High")],
    );
    let options = [model, reasoning];
    let view = SessionConfigView::new(&options);

    assert_eq!(view.current_values(ConfigOptionId::Model), ["image", "audio"]);
    assert_eq!(view.current_display_name(ConfigOptionId::Model).as_deref(), Some("Image + Audio"));
    assert_eq!(view.flattened_options(ConfigOptionId::Model).len(), 2);
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
