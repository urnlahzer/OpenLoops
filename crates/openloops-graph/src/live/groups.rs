//! Resolve only explicitly selected group addresses; never enumerate memberships.
use super::{
    Client, ConnectionError, GRAPH_TIMEOUT_SECONDS, Url, bounded_body, classify_status,
    request_error, valid_application_id,
};

pub(super) fn lookup_url(address: &str) -> Result<Url, ConnectionError> {
    let mut url = Url::parse("https://graph.microsoft.com/v1.0/groups")
        .map_err(|_| ConnectionError::InvalidConfiguration)?;
    let literal = address.replace('\'', "''");
    url.query_pairs_mut()
        .append_pair(
            "$filter",
            &format!("mail eq '{literal}' and groupTypes/any(t:t eq 'Unified')"),
        )
        .append_pair("$select", "id")
        .append_pair("$top", "2");
    Ok(url)
}

pub(super) fn resolve_id(bytes: &[u8]) -> Result<String, ConnectionError> {
    let response: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| ConnectionError::ResourceUnavailable)?;
    let rows = response
        .get("value")
        .and_then(serde_json::Value::as_array)
        .ok_or(ConnectionError::ResourceUnavailable)?;
    if rows.len() != 1 || response.get("@odata.nextLink").is_some() {
        return Err(ConnectionError::GroupNotFound);
    }
    let id = rows[0]
        .get("id")
        .and_then(serde_json::Value::as_str)
        .filter(|value| valid_application_id(value))
        .ok_or(ConnectionError::ResourceUnavailable)?;
    Ok(id.to_owned())
}

pub(super) fn check(http: &Client, token: &str, address: &str) -> Result<(), ConnectionError> {
    let response = http
        .get(lookup_url(address)?)
        .bearer_auth(token)
        .send()
        .map_err(|error| request_error(&error, GRAPH_TIMEOUT_SECONDS))?;
    if response.status().as_u16() != 200 {
        return Err(classify_status(response.status().as_u16()));
    }
    let id = resolve_id(&bounded_body(response)?)?;
    let mut url = Url::parse("https://graph.microsoft.com/v1.0/groups/")
        .map_err(|_| ConnectionError::InvalidConfiguration)?;
    url.path_segments_mut()
        .map_err(|()| ConnectionError::InvalidConfiguration)?
        .pop_if_empty()
        .push(&id)
        .push("threads");
    url.query_pairs_mut()
        .append_pair("$select", "id")
        .append_pair("$top", "1");
    let response = http
        .get(url)
        .bearer_auth(token)
        .send()
        .map_err(|error| request_error(&error, GRAPH_TIMEOUT_SECONDS))?;
    if response.status().as_u16() != 200 {
        return Err(classify_status(response.status().as_u16()));
    }
    let _ = bounded_body(response)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_is_escaped_as_an_odata_literal() {
        let url = lookup_url("o'hare@example.invalid").unwrap();
        assert_eq!(
            url.origin().ascii_serialization(),
            "https://graph.microsoft.com"
        );
        assert!(url.query_pairs().any(|(key, value)| key == "$filter"
            && value == "mail eq 'o''hare@example.invalid' and groupTypes/any(t:t eq 'Unified')"));
    }

    #[test]
    fn lookup_requires_one_valid_group_and_never_follows_returned_links() {
        assert!(
            resolve_id(br#"{"value":[{"id":"11111111-1111-4111-8111-111111111111"}]}"#).is_ok()
        );
        for response in [
            br#"{"value":[]}"#.as_slice(),
            br#"{"value":[{"id":"../me"}]}"#,
            br#"{"value":[{},{}]}"#,
            br#"{"value":[{}],"@odata.nextLink":"https://example.invalid"}"#,
        ] {
            assert!(resolve_id(response).is_err());
        }
    }
}
