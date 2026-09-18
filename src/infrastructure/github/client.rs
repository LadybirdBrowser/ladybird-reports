use chrono::{DateTime, Utc};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use reqwest::{Client, Method, StatusCode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{sync::Arc, time::Duration};

use base64::Engine;

use crate::error::{AppError, Result};

#[derive(Clone)]
pub struct GithubClient {
    http: Client,
    client_id: String,
    client_secret: String,
    api_base_url: reqwest::Url,
    oauth_base_url: reqwest::Url,
    app_private_key: Option<Arc<EncodingKey>>,
}

#[derive(Debug, Deserialize)]
pub struct GithubAccessToken {
    pub access_token: String,
    pub expires_in: Option<i64>,
    pub refresh_token: Option<String>,
    pub refresh_token_expires_in: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct GithubUser {
    pub id: i64,
    pub login: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct GithubIssue {
    pub id: i64,
    pub number: i64,
    pub title: String,
    pub body: Option<String>,
    pub html_url: String,
    pub state: GithubIssueState,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub state_reason: Option<String>,
}

#[derive(Serialize)]
struct AppJwtClaims<'a> {
    iat: i64,
    exp: i64,
    iss: &'a str,
}

#[derive(Deserialize)]
struct InstallationToken {
    token: String,
}

#[derive(Deserialize)]
struct GraphqlResponse<T> {
    data: Option<T>,
    errors: Option<Vec<GraphqlError>>,
}

#[derive(Deserialize)]
struct GraphqlError {
    message: String,
}

#[derive(Deserialize)]
struct DuplicateLookup {
    repository: Option<DuplicateRepository>,
}

#[derive(Deserialize)]
struct DuplicateRepository {
    issue: Option<DuplicateSource>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DuplicateSource {
    state_reason: Option<String>,
    duplicate_of: Option<CanonicalIssue>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CanonicalIssue {
    #[serde(rename = "__typename")]
    type_name: String,
    database_id: Option<i64>,
    number: Option<i64>,
    title: Option<String>,
    body: Option<String>,
    url: Option<String>,
    state: Option<String>,
    updated_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum GithubIssueState {
    Open,
    Closed,
}

impl GithubIssueState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
        }
    }
}

impl GithubIssue {
    pub fn repository(&self) -> Option<String> {
        let url = reqwest::Url::parse(&self.html_url).ok()?;
        if url.scheme() != "https" || url.host_str() != Some("github.com") {
            return None;
        }

        let segments = url.path_segments()?.collect::<Vec<_>>();
        if segments.len() != 4 || segments[2] != "issues" {
            return None;
        }
        if segments[3].parse::<i64>().ok()? != self.number {
            return None;
        }

        Some(format!("{}/{}", segments[0], segments[1]))
    }

    pub fn belongs_to_repository(&self, repository: &str) -> bool {
        self.repository()
            .is_some_and(|value| value.eq_ignore_ascii_case(repository))
    }
}

#[derive(Debug, Deserialize)]
struct GithubIssueSearch {
    items: Vec<GithubIssue>,
}

#[derive(Deserialize)]
struct GithubIssueField {
    id: i64,
    data_type: String,
    visibility: Option<String>,
}

#[derive(Deserialize)]
struct TeamMembership {
    state: String,
}

#[derive(Serialize)]
struct NewIssue<'a> {
    title: &'a str,
    body: &'a str,
}

impl GithubClient {
    pub fn new(client_id: String, client_secret: String) -> Result<Self> {
        let http = Client::builder()
            .user_agent("Ladybird-Reports/0.1")
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|error| AppError::Internal(error.into()))?;

        let api_base_url = std::env::var("GITHUB_API_BASE_URL")
            .unwrap_or_else(|_| "https://api.github.com".into());
        let api_base_url =
            reqwest::Url::parse(&api_base_url).map_err(|error| AppError::Internal(error.into()))?;

        // Browser tests use a local OAuth endpoint. Release builds always use
        // GitHub's endpoint, regardless of the surrounding environment.
        #[cfg(debug_assertions)]
        let oauth_base_url = std::env::var("GITHUB_TEST_OAUTH_BASE_URL")
            .unwrap_or_else(|_| "https://github.com".into());
        #[cfg(not(debug_assertions))]
        let oauth_base_url = "https://github.com".to_owned();
        let oauth_base_url = reqwest::Url::parse(&oauth_base_url)
            .map_err(|error| AppError::Internal(error.into()))?;
        #[cfg(debug_assertions)]
        if std::env::var_os("GITHUB_TEST_OAUTH_BASE_URL").is_some()
            && !matches!(
                oauth_base_url.host_str(),
                Some("127.0.0.1" | "localhost" | "[::1]" | "::1")
            )
        {
            return Err(AppError::InvalidRequest(
                "Test OAuth endpoint must be on the local machine",
            ));
        }

        let app_private_key = std::env::var("GITHUB_APP_PRIVATE_KEY_BASE64")
            .ok()
            .map(|encoded| {
                let pem = base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .map_err(|error| AppError::Internal(error.into()))?;
                let key = EncodingKey::from_rsa_pem(&pem)
                    .map_err(|error| AppError::Internal(error.into()))?;
                Ok::<_, AppError>(Arc::new(key))
            })
            .transpose()?;

        Ok(Self {
            http,
            client_id,
            client_secret,
            api_base_url,
            oauth_base_url,
            app_private_key,
        })
    }

    pub fn has_installation_credentials(&self) -> bool {
        self.app_private_key.is_some()
    }

    async fn installation_token(&self, installation_id: i64) -> Result<String> {
        let key = self.app_private_key.as_ref().ok_or(AppError::Unavailable)?;
        let now = Utc::now().timestamp();
        let jwt = encode(
            &Header::new(Algorithm::RS256),
            &AppJwtClaims {
                iat: now - 60,
                exp: now + 540,
                iss: &self.client_id,
            },
            key,
        )
        .map_err(|error| AppError::Internal(error.into()))?;
        let path = format!("/app/installations/{installation_id}/access_tokens");
        let result: InstallationToken = self
            .request_json(
                Method::POST,
                &path,
                &jwt,
                Some(&serde_json::json!({ "permissions": { "issues": "read" } })),
            )
            .await?;
        Ok(result.token)
    }

    pub async fn duplicate_of(
        &self,
        installation_id: i64,
        repository: &str,
        number: i64,
    ) -> Result<Option<GithubIssue>> {
        let (owner, name) = repository
            .split_once('/')
            .ok_or(AppError::InvalidRequest("Invalid GitHub repository"))?;
        let token = self.installation_token(installation_id).await?;
        let query = serde_json::json!({
            "query": r#"
                query($owner: String!, $name: String!, $number: Int!) {
                    repository(owner: $owner, name: $name) {
                        issue(number: $number) {
                            stateReason
                            duplicateOf {
                                __typename
                                ... on Issue {
                                    databaseId number title body url state updatedAt
                                }
                            }
                        }
                    }
                }
            "#,
            "variables": { "owner": owner, "name": name, "number": number },
        });
        let url = self.api_url("/graphql")?;
        let response: GraphqlResponse<DuplicateLookup> = self
            .request_json_url(Method::POST, url, &token, Some(&query))
            .await?;
        if let Some(errors) = response.errors {
            tracing::warn!(
                event = "github.duplicate_lookup_failed",
                errors = ?errors.iter().map(|error| &error.message).collect::<Vec<_>>()
            );
            return Err(AppError::Unavailable);
        }
        let source = response
            .data
            .and_then(|data| data.repository)
            .and_then(|repository| repository.issue)
            .ok_or(AppError::NotFound("GitHub issue not found"))?;
        if source.state_reason.as_deref() != Some("DUPLICATE") {
            return Ok(None);
        }
        let canonical = source.duplicate_of.ok_or(AppError::Unavailable)?;
        if canonical.type_name != "Issue" {
            return Err(AppError::Conflict("Duplicate target is not a GitHub issue"));
        }
        let state = match canonical.state.as_deref() {
            Some("OPEN") => GithubIssueState::Open,
            Some("CLOSED") => GithubIssueState::Closed,
            _ => return Err(AppError::Unavailable),
        };
        Ok(Some(GithubIssue {
            id: canonical.database_id.ok_or(AppError::Unavailable)?,
            number: canonical.number.ok_or(AppError::Unavailable)?,
            title: canonical.title.ok_or(AppError::Unavailable)?,
            body: canonical.body,
            html_url: canonical.url.ok_or(AppError::Unavailable)?,
            state,
            updated_at: canonical.updated_at.ok_or(AppError::Unavailable)?,
            state_reason: None,
        }))
    }

    pub fn authorization_url(&self, redirect_uri: &str, state: &str) -> Result<String> {
        let mut url = self
            .oauth_base_url
            .join("/login/oauth/authorize")
            .expect("OAuth base URL is valid");

        url.query_pairs_mut()
            .append_pair("client_id", &self.client_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("state", state);

        // GitHub App user access tokens derive their access from the app's
        // fine-grained permissions. Adding an OAuth scope here would request
        // the much broader permissions used by legacy OAuth Apps.

        Ok(url.to_string())
    }

    pub async fn exchange_code(&self, code: &str) -> Result<GithubAccessToken> {
        let response = self
            .http
            .post(
                self.oauth_base_url
                    .join("/login/oauth/access_token")
                    .expect("OAuth base URL is valid"),
            )
            .header("accept", "application/json")
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("code", code),
            ])
            .send()
            .await
            .map_err(|error| AppError::Internal(error.into()))?;

        decode_github_response(response).await
    }

    pub async fn refresh_user_token(&self, refresh_token: &str) -> Result<GithubAccessToken> {
        let response = self
            .http
            .post(
                self.oauth_base_url
                    .join("/login/oauth/access_token")
                    .expect("OAuth base URL is valid"),
            )
            .header("accept", "application/json")
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
            ])
            .send()
            .await
            .map_err(|error| AppError::Internal(error.into()))?;

        let status = response.status();
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|error| AppError::Internal(error.into()))?;

        if body.get("error").and_then(serde_json::Value::as_str) == Some("bad_refresh_token") {
            return Err(AppError::AuthenticationRequired);
        }

        if !status.is_success() || body.get("error").is_some() {
            return Err(AppError::Unavailable);
        }

        serde_json::from_value(body).map_err(|error| AppError::Internal(error.into()))
    }

    pub async fn current_user(&self, token: &str) -> Result<GithubUser> {
        self.request_json(Method::GET, "/user", token, Option::<&()>::None)
            .await
    }

    pub async fn verify_team_membership(
        &self,
        token: &str,
        authorization_team: &str,
        login: &str,
    ) -> Result<()> {
        let path = team_membership_path(authorization_team, login);
        let membership: TeamMembership = self
            .request_json(Method::GET, &path, token, Option::<&()>::None)
            .await
            .map_err(|error| match error {
                AppError::NotFound(_) | AppError::PermissionDenied(_) => {
                    AppError::PermissionDenied("Access denied")
                }
                other => other,
            })?;

        if membership.state != "active" {
            return Err(AppError::PermissionDenied("Access denied"));
        }

        Ok(())
    }

    pub async fn verify_team_member_identity(
        &self,
        token: &str,
        authorization_team: &str,
        github_id: i64,
    ) -> Result<()> {
        let user = self.current_user(token).await?;
        if user.id != github_id {
            return Err(AppError::PermissionDenied("Access denied"));
        }

        self.verify_team_membership(token, authorization_team, &user.login)
            .await
    }

    pub async fn issue(&self, token: &str, repository: &str, number: i64) -> Result<GithubIssue> {
        let path = format!("/repos/{repository}/issues/{number}");
        self.request_json(Method::GET, &path, token, Option::<&()>::None)
            .await
    }

    pub async fn search_issues(
        &self,
        token: &str,
        repository: &str,
        query: &str,
    ) -> Result<Vec<GithubIssue>> {
        let mut url = self.api_url("/search/issues")?;
        url.query_pairs_mut()
            .append_pair("q", &format!("repo:{repository} is:issue {query}"))
            .append_pair("per_page", "20");

        let response: GithubIssueSearch = self
            .request_json_url(Method::GET, url, token, Option::<&()>::None)
            .await?;
        Ok(response.items)
    }

    pub async fn create_issue(
        &self,
        token: &str,
        repository: &str,
        title: &str,
        body: &str,
    ) -> Result<GithubIssue> {
        let path = format!("/repos/{repository}/issues");
        self.request_json(Method::POST, &path, token, Some(&NewIssue { title, body }))
            .await
    }

    pub async fn verify_private_text_issue_field(
        &self,
        token: &str,
        organization: &str,
        field_id: i64,
    ) -> Result<()> {
        let path = format!("/orgs/{organization}/issue-fields");
        let url = self.api_url(&path)?;
        let fields: Vec<GithubIssueField> = self
            .request_json_url_version(Method::GET, url, token, Option::<&()>::None, "2026-03-10")
            .await?;
        let field = fields
            .into_iter()
            .find(|field| field.id == field_id)
            .ok_or(AppError::Conflict("GitHub Reports field was not found"))?;

        if field.data_type != "text"
            || field.visibility.as_deref() != Some("organization_members_only")
        {
            return Err(AppError::Conflict(
                "GitHub Reports field must be an organization-only text field",
            ));
        }

        Ok(())
    }

    pub async fn add_issue_field_link(
        &self,
        token: &str,
        repository: &str,
        issue_number: i64,
        field_id: i64,
        link: &str,
    ) -> Result<()> {
        let path = format!("/repos/{repository}/issues/{issue_number}/issue-field-values");
        let url = self.api_url(&path)?;
        let _: serde_json::Value = self
            .request_json_url_version(
                Method::POST,
                url,
                token,
                Some(&serde_json::json!({
                    "issue_field_values": [{ "field_id": field_id, "value": link }]
                })),
                "2026-03-10",
            )
            .await?;
        Ok(())
    }

    async fn request_json<T, B>(
        &self,
        method: Method,
        path: &str,
        token: &str,
        body: Option<&B>,
    ) -> Result<T>
    where
        T: DeserializeOwned,
        B: Serialize + ?Sized,
    {
        let url = self.api_url(path)?;
        self.request_json_url(method, url, token, body).await
    }

    fn api_url(&self, path: &str) -> Result<reqwest::Url> {
        self.api_base_url
            .join(path)
            .map_err(|error| AppError::Internal(error.into()))
    }

    async fn request_json_url<T, B>(
        &self,
        method: Method,
        url: reqwest::Url,
        token: &str,
        body: Option<&B>,
    ) -> Result<T>
    where
        T: DeserializeOwned,
        B: Serialize + ?Sized,
    {
        self.request_json_url_version(method, url, token, body, "2022-11-28")
            .await
    }

    async fn request_json_url_version<T, B>(
        &self,
        method: Method,
        url: reqwest::Url,
        token: &str,
        body: Option<&B>,
        api_version: &str,
    ) -> Result<T>
    where
        T: DeserializeOwned,
        B: Serialize + ?Sized,
    {
        let mut request = self
            .http
            .request(method, url)
            .bearer_auth(token)
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", api_version);

        if let Some(body) = body {
            request = request.json(body);
        }

        let response = request
            .send()
            .await
            .map_err(|error| AppError::Internal(error.into()))?;

        decode_github_response(response).await
    }
}

fn team_membership_path(authorization_team: &str, login: &str) -> String {
    let (organization, team_slug) = authorization_team
        .split_once('/')
        .expect("validated GitHub access team has an organization and slug");
    format!("/orgs/{organization}/teams/{team_slug}/memberships/{login}")
}

async fn decode_github_response<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    match response.status() {
        status if status.is_success() => response
            .json()
            .await
            .map_err(|error| AppError::Internal(error.into())),
        StatusCode::NOT_FOUND => Err(AppError::NotFound("GitHub resource not found")),
        StatusCode::GONE => Err(AppError::Gone("GitHub issue was deleted")),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            Err(AppError::PermissionDenied("GitHub denied the request"))
        }
        StatusCode::TOO_MANY_REQUESTS => Err(AppError::RateLimited),
        StatusCode::UNPROCESSABLE_ENTITY => Err(AppError::InvalidRequest(
            "GitHub rejected the issue contents",
        )),
        _ => Err(AppError::Unavailable),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{DuplicateLookup, GithubClient, GraphqlResponse, team_membership_path};

    #[test]
    fn github_duplicate_lookup_includes_the_canonical_issue() {
        let response: GraphqlResponse<DuplicateLookup> =
            serde_json::from_value(serde_json::json!({
                "data": {
                    "repository": {
                        "issue": {
                            "stateReason": "DUPLICATE",
                            "duplicateOf": {
                                "__typename": "Issue",
                                "databaseId": 123,
                                "number": 42,
                                "title": "Canonical issue",
                                "body": "Details",
                                "url": "https://github.com/LadybirdBrowser/ladybird/issues/42",
                                "state": "OPEN",
                                "updatedAt": "2026-09-18T08:00:00Z"
                            }
                        }
                    }
                }
            }))
            .expect("decode GitHub GraphQL response");

        let source = response.data.unwrap().repository.unwrap().issue.unwrap();
        assert_eq!(source.state_reason.as_deref(), Some("DUPLICATE"));
        let target = source.duplicate_of.unwrap();
        assert_eq!(target.number, Some(42));
        assert_eq!(target.title.as_deref(), Some("Canonical issue"));
    }

    #[test]
    fn membership_path_uses_the_configured_team() {
        assert_eq!(
            team_membership_path("ExampleOrg/reports-reviewers", "reviewer"),
            "/orgs/ExampleOrg/teams/reports-reviewers/memberships/reviewer"
        );
    }

    #[test]
    fn authorization_url_does_not_request_legacy_oauth_scopes() {
        let client = GithubClient::new("client-id".into(), "client-secret".into())
            .expect("create GitHub client");
        let authorization_url = client
            .authorization_url("https://reports.example/auth/callback", "state-token")
            .expect("build authorization URL");
        let authorization_url =
            reqwest::Url::parse(&authorization_url).expect("parse authorization URL");
        let parameters = authorization_url
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect::<HashMap<_, _>>();

        assert_eq!(
            authorization_url.as_str().split('?').next(),
            Some("https://github.com/login/oauth/authorize")
        );
        assert_eq!(
            parameters.get("client_id").map(String::as_str),
            Some("client-id")
        );
        assert_eq!(
            parameters.get("redirect_uri").map(String::as_str),
            Some("https://reports.example/auth/callback")
        );
        assert_eq!(
            parameters.get("state").map(String::as_str),
            Some("state-token")
        );
        assert!(!parameters.contains_key("scope"));
    }
}
