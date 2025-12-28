use dioxus::prelude::*;

use super::voice_input::VoiceInput;

#[component]
pub fn PromptInput(
    value: Signal<String>,
    on_keydown: EventHandler<KeyboardEvent>,
    placeholder: String,
    disabled: bool,
    rows: String,
) -> Element {
    let on_input = move |e: FormEvent| {
        value.set(e.value().clone());
    };

    let on_voice_transcription = move |text: String| {
        // Append transcribed text to the input
        let current = value.read().clone();
        let new_value = if current.is_empty() {
            text
        } else {
            format!("{} {}", current, text)
        };
        value.set(new_value);
    };

    rsx! {
        div {
            class: "flex flex-col gap-2",
            div {
                class: "flex gap-3",
                textarea {
                    class: "input-field flex-1 rounded-xl px-4 py-3 resize-none",
                    value: "{value}",
                    oninput: on_input,
                    onkeydown: move |e| on_keydown.call(e),
                    placeholder: "{placeholder}",
                    disabled: disabled,
                    rows: "{rows}",
                }
            }
            div {
                class: "flex justify-end",
                VoiceInput {
                    on_transcription: on_voice_transcription,
                    disabled: disabled,
                }
            }
        }
    }
}
