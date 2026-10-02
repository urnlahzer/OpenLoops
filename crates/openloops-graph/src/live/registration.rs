use super::google::{valid_client_id, valid_client_secret};
use super::valid_application_id;

pub struct MicrosoftRegistration {
    pub client_id: &'static str,
}

pub struct GoogleRegistration {
    pub client_id: &'static str,
    pub client_secret: &'static str,
}

#[must_use]
pub fn microsoft() -> Option<MicrosoftRegistration> {
    microsoft_from(option_env!("OPENLOOPS_MS_CLIENT_ID"))
}

#[must_use]
pub fn google() -> Option<GoogleRegistration> {
    google_from(
        option_env!("OPENLOOPS_GOOGLE_CLIENT_ID"),
        option_env!("OPENLOOPS_GOOGLE_CLIENT_SECRET"),
    )
}

fn microsoft_from(value: Option<&'static str>) -> Option<MicrosoftRegistration> {
    value
        .filter(|value| valid_application_id(value))
        .map(|client_id| MicrosoftRegistration { client_id })
}

fn google_from(
    client_id: Option<&'static str>,
    client_secret: Option<&'static str>,
) -> Option<GoogleRegistration> {
    match (client_id, client_secret) {
        (Some(client_id), Some(client_secret))
            if valid_client_id(client_id) && valid_client_secret(client_secret) =>
        {
            Some(GoogleRegistration {
                client_id,
                client_secret,
            })
        }
        _ => None,
    }
}

pub fn effective_client_id(byo_field: &str, shipped: Option<&str>) -> Option<String> {
    let byo = byo_field.trim();
    if byo.is_empty() {
        shipped.map(str::to_owned)
    } else {
        Some(byo.to_owned())
    }
}

#[must_use]
/// Builds the organization-wide consent URL. After consent, the browser may land on an
/// unreachable localhost page; the consent is still recorded by Microsoft.
pub fn microsoft_admin_consent_url(client_id: &str) -> String {
    const SCOPES: [&str; 6] = [
        "User.Read",
        "Mail.Read",
        "Mail.Read.Shared",
        "Tasks.ReadWrite",
        "Group.ReadBasic.All",
        "Group-Conversation.Read.All",
    ];
    let scopes = SCOPES
        .map(|scope| format!("https://graph.microsoft.com/{scope}"))
        .join(" ");
    let encoded_scopes: String = url::form_urlencoded::byte_serialize(scopes.as_bytes()).collect();
    let encoded_id: String = url::form_urlencoded::byte_serialize(client_id.as_bytes()).collect();
    format!(
        "https://login.microsoftonline.com/organizations/v2.0/adminconsent?client_id={encoded_id}&scope={encoded_scopes}&redirect_uri=http%3A%2F%2Flocalhost"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byo_precedes_shipped_and_empty_values_fall_back() {
        assert_eq!(
            effective_client_id(" byo ", Some("shipped")),
            Some("byo".into())
        );
        assert_eq!(
            effective_client_id("  ", Some("shipped")),
            Some("shipped".into())
        );
        assert_eq!(effective_client_id("", None), None);
    }

    #[test]
    fn invalid_shipped_registrations_are_absent() {
        assert!(microsoft_from(Some("not-an-application-id")).is_none());
        assert!(google_from(Some("invalid"), Some("fixture-secret")).is_none());
        assert!(google_from(Some("123-fixture.apps.googleusercontent.com"), None).is_none());
    }

    #[test]
    fn admin_consent_url_contains_the_complete_encoded_scope_union() {
        let url = microsoft_admin_consent_url("11111111-1111-4111-8111-111111111111");
        assert!(url.starts_with("https://login.microsoftonline.com/organizations/v2.0/adminconsent?client_id=11111111-1111-4111-8111-111111111111&scope="));
        for scope in [
            "User.Read",
            "Mail.Read",
            "Mail.Read.Shared",
            "Tasks.ReadWrite",
            "Group.ReadBasic.All",
            "Group-Conversation.Read.All",
        ] {
            assert!(url.contains(&format!("https%3A%2F%2Fgraph.microsoft.com%2F{scope}")));
        }
        assert!(url.ends_with("&redirect_uri=http%3A%2F%2Flocalhost"));
    }
}
