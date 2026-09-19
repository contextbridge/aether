use std::io::{self, IsTerminal, Read as _};

pub(crate) fn prompt_or_stdin(explicit: Option<String>) -> io::Result<Option<String>> {
    if let Some(prompt) = explicit {
        return Ok(non_empty_prompt(&prompt));
    }
    if io::stdin().is_terminal() {
        return Ok(None);
    }

    let mut prompt = String::new();
    io::stdin().read_to_string(&mut prompt)?;
    Ok(non_empty_prompt(&prompt))
}

fn non_empty_prompt(prompt: &str) -> Option<String> {
    let prompt = prompt.trim();
    (!prompt.is_empty()).then(|| prompt.to_string())
}
