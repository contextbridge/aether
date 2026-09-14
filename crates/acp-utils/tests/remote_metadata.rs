use acp_utils::notifications::{AETHER_META_NAMESPACE, RemoteServerInfo};
use agent_client_protocol::schema::v2::Meta;
use serde_json::json;

#[test]
fn remote_metadata_round_trips_with_and_without_a_session() {
    for session_id in [None, Some("live".into())] {
        let info = RemoteServerInfo { cwd: "/server/workspace".into(), session_id };
        let meta = info.to_meta();
        assert_eq!(
            serde_json::to_value(&meta).unwrap(),
            json!({
                "contextbridge/aether": { "remote": { "cwd": "/server/workspace", "sessionId": info.session_id } }
            })
        );
        assert_eq!(RemoteServerInfo::from_meta(Some(&meta)), Some(info));
    }
}

#[test]
fn remote_metadata_requires_a_valid_contract() {
    assert_eq!(RemoteServerInfo::from_meta(None), None);
    for value in
        [json!({}), json!({"remote": null}), json!({"remote": {"sessionId": "live"}}), json!({"remote": {"cwd": 42}})]
    {
        let meta = Meta::from_iter([(AETHER_META_NAMESPACE.into(), value)]);
        assert_eq!(RemoteServerInfo::from_meta(Some(&meta)), None);
    }
}
