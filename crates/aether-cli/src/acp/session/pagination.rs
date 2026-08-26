use aether_sessions::SessionSummary;
use agent_client_protocol::schema::v1 as acp;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

const SESSION_PAGE_SIZE: usize = 50;

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

#[derive(Debug, Deserialize, Serialize)]
struct SessionListCursor {
    created_at: String,
    session_id: String,
}

fn encode_cursor(summary: &SessionSummary) -> String {
    let cursor =
        SessionListCursor { created_at: summary.meta.created_at.clone(), session_id: summary.meta.session_id.clone() };
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(&cursor).expect("session list cursor is serializable"))
}

fn decode_cursor(value: &str) -> Result<SessionListCursor, acp::Error> {
    let bytes = URL_SAFE_NO_PAD.decode(value).map_err(|_| acp::Error::invalid_params())?;
    serde_json::from_slice(&bytes).map_err(|_| acp::Error::invalid_params())
}
