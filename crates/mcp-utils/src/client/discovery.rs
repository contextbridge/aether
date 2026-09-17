use rmcp::{
    Peer, RoleClient,
    model::{PaginatedRequestParams, RequestMetaObject, Tool},
    service::ServiceError,
};

/// Carry the same request-scoped policy through every discovery page.
pub async fn list_all_tools(
    peer: &Peer<RoleClient>,
    meta: Option<RequestMetaObject>,
) -> Result<Vec<Tool>, ServiceError> {
    let mut tools = Vec::new();
    let mut cursor = None;
    loop {
        let mut params = PaginatedRequestParams::default().with_cursor(cursor);
        params.meta = meta.clone();
        let page = peer.list_tools(Some(params)).await?;
        tools.extend(page.tools);
        cursor = page.next_cursor;
        if cursor.is_none() {
            return Ok(tools);
        }
    }
}
