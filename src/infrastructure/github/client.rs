use reqwest::{Client, Method, StatusCode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::error::{AppError, Result};

#[derive(Clone)]
pub struct GithubClient {
    http: Client,
    client_id: String,
    client_secret: String,
}

#[derive(Debug, Deserialize)]
pub struct GithubAccessToken {
    pub access_token: String,
    pub expires_in: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct GithubUser {
    pub id: i64,
    pub login: String,
}

#[derive(Debug, Deserialize)]
pub struct GithubIssue {
    pub number: i64,
    pub title: String,
    pub html_url: String,
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
            .build()
            .map_err(|error| AppError::Internal(error.into()))?;

        Ok(Self {
            http,
            client_id,
            client_secret,
        })
    }

    pub fn authorization_url(&self, redirect_uri: &str, state: &str) -> Result<String> {
        let mut url = reqwest::Url::parse("https://github.com/login/oauth/authorize")
            .expect("static GitHub URL is valid");

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
            .post("https://github.com/login/oauth/access_token")
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

    pub async fn current_user(&self, token: &str) -> Result<GithubUser> {
        self.request_json(Method::GET, "/user", token, Option::<&()>::None)
            .await
    }

    pub async fn verify_maintainer(&self, token: &str, login: &str) -> Result<()> {
        let path = format!("/orgs/LadybirdBrowser/teams/maintainers/memberships/{login}");
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

    pub async fn issue(&self, token: &str, repository: &str, number: i64) -> Result<GithubIssue> {
        let path = format!("/repos/{repository}/issues/{number}");
        self.request_json(Method::GET, &path, token, Option::<&()>::None)
            .await
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
        let mut request = self
            .http
            .request(method, format!("https://api.github.com{path}"))
            .bearer_auth(token)
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28");

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

async fn decode_github_response<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    match response.status() {
        status if status.is_success() => response
            .json()
            .await
            .map_err(|error| AppError::Internal(error.into())),
        StatusCode::NOT_FOUND => Err(AppError::NotFound("GitHub resource not found")),
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

    use super::GithubClient;

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
