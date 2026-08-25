use aether_sessions::SessionSummary;
use agent_client_protocol::schema::v1 as acp;
use serde::{Deserialize, Serialize};

const SESSION_PAGE_SIZE: usize = 50;

#[derive(Debug, Deserialize, Serialize)]
struct SessionListCursor {
    created_at: String,
    session_id: String,
}

pub(crate) fn paginate_summaries(
    summaries: Vec<SessionSummary>,
    cursor: Option<&str>,
) -> Result<(Vec<SessionSummary>, Option<String>), acp::Error> {
    let start = match cursor {
        Some(cursor) => {
            let cursor = decode_cursor(cursor)?;
            summaries
                .iter()
                .position(|summary| {
                    summary.meta.created_at == cursor.created_at && summary.meta.session_id == cursor.session_id
                })
                .map(|index| index + 1)
                .ok_or_else(acp::Error::invalid_params)?
        }
        None => 0,
    };

    let end = start.saturating_add(SESSION_PAGE_SIZE).min(summaries.len());
    let next_cursor = (end < summaries.len()).then(|| encode_cursor(&summaries[end - 1]));
    Ok((summaries.into_iter().skip(start).take(end - start).collect(), next_cursor))
}

fn encode_cursor(summary: &SessionSummary) -> String {
    let cursor =
        SessionListCursor { created_at: summary.meta.created_at.clone(), session_id: summary.meta.session_id.clone() };
    serde_json::to_vec(&cursor)
        .expect("session list cursor is serializable")
        .into_iter()
        .flat_map(|byte| [hex_digit(byte >> 4), hex_digit(byte & 0x0f)])
        .collect()
}

fn decode_cursor(value: &str) -> Result<SessionListCursor, acp::Error> {
    if value.is_empty() || !value.len().is_multiple_of(2) {
        return Err(acp::Error::invalid_params());
    }

    let bytes = value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| hex_value(pair[0]).and_then(|high| hex_value(pair[1]).map(|low| (high << 4) | low)))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(acp::Error::invalid_params)?;

    serde_json::from_slice(&bytes).map_err(|_| acp::Error::invalid_params())
}

fn hex_digit(value: u8) -> char {
    char::from(b"0123456789abcdef"[value as usize])
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}
