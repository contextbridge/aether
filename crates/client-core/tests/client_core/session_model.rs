use crate::support::{deliver, notices};
use acp_utils::config_meta::SelectOptionMeta;
use acp_utils::conversation::NoticeLevel;
use acp_utils::testing::{model_choice, model_picker, option_group};
use agent_client_protocol::schema::v2::{self as acp, ContentBlock};
use client_core::command::CommandResult;
use client_core::composer::attachment::{AttachmentOutcome, PromptAttachment};
use client_core::composer::submission::Submission;
use client_core::session_core::SessionCore;
use client_core::testing::SessionCoreConfigBuilder;

#[test]
fn media_is_sent_only_when_the_agent_and_every_selected_model_take_it() {
    let no_image = acp::PromptCapabilities::new().audio(acp::PromptAudioCapabilities::new());
    let no_audio = acp::PromptCapabilities::new().image(acp::PromptImageCapabilities::new());
    let cases = [
        (no_image, vec![], image(), Some("ACP agent does not support image input.")),
        (no_audio, vec![], audio(), Some("ACP agent does not support audio input.")),
        (media(), models("gpt", &[("gpt", false, false)]), image(), Some(NO_IMAGE)),
        (media(), models("gpt", &[("gpt", true, false)]), audio(), Some(NO_AUDIO)),
        (media(), models("unknown", &[("known", true, true)]), image(), Some(NO_METADATA)),
        (media(), grouped("a", &[("a", false, true)]), image(), Some(NO_IMAGE)),
        (media(), grouped("a", &[("a", true, false)]), audio(), Some(NO_AUDIO)),
        (media(), grouped("a,b", &[("a", true, true), ("b", false, false)]), image(), Some(NO_IMAGE)),
        (media(), vec![], image(), None),
        (media(), models("a", &[("a", true, true)]), image(), None),
        (media(), grouped("a,b", &[("a", true, true), ("b", true, true)]), image(), None),
        (acp::PromptCapabilities::new(), models("gpt", &[("gpt", false, false)]), "text".into(), None),
    ];
    let attachment = PromptAttachment { path: "/workspace/attached".into(), display_name: "attached".into() };
    for (capabilities, config_options, block, expected) in cases {
        let config = SessionCoreConfigBuilder::new().prompt_capabilities(capabilities).config_options(config_options);
        let mut core = SessionCore::new(config.build());
        core.submit(Submission { media: vec![attachment.clone()], ..Submission::from("look") });

        let prepared = AttachmentOutcome { blocks: vec![block.clone()], warnings: vec![] };
        deliver(&mut core, CommandResult::SubmissionPrepared(prepared));

        assert_eq!(notices(&core, NoticeLevel::Error), Vec::from_iter(expected), "{block:?}");
        assert_eq!(core.queued_prompts().is_empty(), expected.is_some(), "{block:?}");
    }
}

const NO_IMAGE: &str = "Current model selection does not support image input.";
const NO_AUDIO: &str = "Current model selection does not support audio input.";
const NO_METADATA: &str = "Current model selection is missing prompt capability metadata.";

fn media() -> acp::PromptCapabilities {
    acp::PromptCapabilities::new().image(acp::PromptImageCapabilities::new()).audio(acp::PromptAudioCapabilities::new())
}

fn image() -> ContentBlock {
    ContentBlock::Image(acp::ImageContent::new("aGk=", "image/png"))
}

fn audio() -> ContentBlock {
    ContentBlock::Audio(acp::AudioContent::new("aGk=", "audio/wav"))
}

fn models(current: &str, offered: &[(&str, bool, bool)]) -> Vec<acp::SessionConfigOption> {
    vec![model_picker(
        current,
        offered.iter().map(|&(value, image, audio)| model(value, image, audio)).collect::<Vec<_>>(),
    )]
}

fn grouped(current: &str, offered: &[(&str, bool, bool)]) -> Vec<acp::SessionConfigOption> {
    let groups: Vec<_> =
        offered.iter().map(|&(value, image, audio)| option_group(value, vec![model(value, image, audio)])).collect();
    vec![model_picker(current, groups)]
}

fn model(value: &str, supports_image: bool, supports_audio: bool) -> acp::SessionConfigSelectOption {
    model_choice(value, value, SelectOptionMeta { supports_image, supports_audio, ..Default::default() })
}
