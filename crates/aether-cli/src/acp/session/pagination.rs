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

#[cfg(test)]
mod tests {
    use super::*;
    use aether_sessions::SessionMeta;
    use std::path::PathBuf;

    fn summary(id: &str) -> SessionSummary {
        SessionSummary {
            meta: SessionMeta {
                session_id: id.to_string(),
                cwd: PathBuf::from("/tmp/project"),
                model: "test-model".to_string(),
                selected_mode: None,
                created_at: format!("2026-01-01T00:00:00Z-{id}"),
            },
            title: None,
        }
    }

    fn summaries(count: usize) -> Vec<SessionSummary> {
        (0..count).map(|index| summary(&format!("session-{index:03}"))).collect()
    }

    #[test]
    fn cursor_resumes_after_the_referenced_session() {
        let all = summaries(SESSION_PAGE_SIZE + 2);

        let (first, cursor) = paginate_summaries(all.clone(), None).expect("first page");
        assert_eq!(first.len(), SESSION_PAGE_SIZE);
        let cursor = cursor.expect("more sessions remain");

        let (second, next) = paginate_summaries(all, Some(&cursor)).expect("second page");
        assert_eq!(
            second.iter().map(|summary| summary.meta.session_id.as_str()).collect::<Vec<_>>(),
            ["session-050", "session-051"]
        );
        assert!(next.is_none());
    }

    #[test]
    fn cursor_at_the_last_session_yields_an_empty_terminal_page() {
        let all = summaries(3);
        let cursor = encode_cursor(&all[2]);

        let (page, next) = paginate_summaries(all, Some(&cursor)).expect("terminal page");
        assert!(page.is_empty());
        assert!(next.is_none());
    }

    #[test]
    fn malformed_base64_cursor_is_rejected() {
        assert!(paginate_summaries(summaries(1), Some("not base64!")).is_err());
    }

    #[test]
    fn well_formed_cursor_that_is_not_json_is_rejected() {
        let cursor = URL_SAFE_NO_PAD.encode(b"not a cursor");
        assert!(paginate_summaries(summaries(1), Some(&cursor)).is_err());
    }

    #[test]
    fn cursor_for_a_session_no_longer_listed_is_rejected() {
        let cursor = encode_cursor(&summary("deleted"));
        assert!(paginate_summaries(summaries(2), Some(&cursor)).is_err());
    }
}
