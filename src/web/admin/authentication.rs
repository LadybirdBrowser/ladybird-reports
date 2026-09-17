use askama::Template;
use axum::{
    Extension,
    extract::{Query, State},
    http::{HeaderMap, Uri, header::SET_COOKIE},
    response::{IntoResponse, Redirect, Response},
};
use chrono::Duration;
use serde::Deserialize;

use crate::{
    error::{AppError, Result},
    infrastructure::{hash_secret, random_token},
};

use super::{
    AdminState, TemplateResponse,
    session::{Session, cookie_from_headers},
};

#[derive(Template)]
#[template(path = "login.html")]
pub struct LoginTemplate {
    navigation: Option<Navigation>,
    application_version: &'static str,
    github_login_url: String,
}

#[derive(Clone)]
pub struct Navigation {
    pub login: String,
    pub csrf_token: String,
    pub application_version: &'static str,
    pub bootstrap_reporting_database_url: Option<String>,
}

impl Navigation {
    pub fn for_session(state: &AdminState, session: &Session) -> Self {
        Self {
            login: session.login.clone(),
            csrf_token: session.csrf_token.clone(),
            application_version: crate::runtime::APPLICATION_VERSION,
            bootstrap_reporting_database_url: state
                .bootstrap_reporting_database_url
                .as_deref()
                .map(str::to_owned),
        }
    }
}

#[derive(Default, Deserialize)]
pub struct ReturnToQuery {
    next: Option<String>,
}

pub async fn login(Query(query): Query<ReturnToQuery>) -> TemplateResponse<LoginTemplate> {
    let github_login_url = query
        .next
        .as_deref()
        .and_then(validated_return_to)
        .map(|path| url_with_next("/auth/github", path))
        .unwrap_or_else(|| "/auth/github".to_owned());

    TemplateResponse(LoginTemplate {
        navigation: None,
        application_version: crate::runtime::APPLICATION_VERSION,
        github_login_url,
    })
}

pub async fn start_github_login(
    State(state): State<AdminState>,
    Query(query): Query<ReturnToQuery>,
) -> Result<Response> {
    let configuration = state.database.configuration().await?;
    let state_token = random_token();
    let return_to = query
        .next
        .as_deref()
        .and_then(validated_return_to)
        .unwrap_or("/");

    state
        .database
        .store_oauth_state(&hash_secret(&state_token), return_to)
        .await?;

    let redirect_uri = format!(
        "{}/auth/callback",
        configuration.admin_base_url.trim_end_matches('/')
    );
    let authorization_url = state
        .github
        .authorization_url(&redirect_uri, &state_token)?;

    let secure = configuration.admin_base_url.starts_with("https:");
    let mut response = Redirect::to(&authorization_url).into_response();
    response.headers_mut().append(
        SET_COOKIE,
        session_cookie("oauth_state", &state_token, 600, secure)
            .parse()
            .expect("generated cookie is valid"),
    );

    Ok(response)
}

#[derive(Deserialize)]
pub struct GithubCallback {
    code: String,
    state: String,
}

pub async fn github_callback(
    State(state): State<AdminState>,
    headers: HeaderMap,
    Query(callback): Query<GithubCallback>,
) -> Result<Response> {
    let cookie_state = cookie_from_headers(&headers, "oauth_state");

    if callback.state.len() > 128
        || callback.code.len() > 512
        || cookie_state.as_deref() != Some(callback.state.as_str())
    {
        return Err(AppError::PermissionDenied("Invalid GitHub sign-in state"));
    }

    let return_to = state
        .database
        .consume_oauth_state(&hash_secret(&callback.state))
        .await?;

    let Some(return_to) = return_to else {
        return Err(AppError::PermissionDenied(
            "GitHub sign-in state expired or was already used",
        ));
    };

    let token = state.github.exchange_code(&callback.code).await?;
    let user = state.github.current_user(&token.access_token).await?;

    state
        .github
        .verify_maintainer(&token.access_token, &user.login)
        .await?;

    let session_token = random_token();
    let csrf_token = random_token();
    let lifetime_seconds = token
        .expires_in
        .unwrap_or(8 * 60 * 60)
        .clamp(60, 8 * 60 * 60);

    state
        .database
        .create_session(
            user.id,
            &user.login,
            &hash_secret(&session_token),
            &state.secret_cipher.encrypt(&token.access_token)?,
            &csrf_token,
            Duration::seconds(lifetime_seconds),
        )
        .await?;

    let configuration = state.database.configuration().await?;
    let secure = configuration.admin_base_url.starts_with("https:");
    let return_to = validated_return_to(&return_to).unwrap_or("/");
    let mut response = Redirect::to(return_to).into_response();

    response.headers_mut().append(
        SET_COOKIE,
        session_cookie("session", &session_token, lifetime_seconds, secure)
            .parse()
            .expect("generated cookie is valid"),
    );
    response.headers_mut().append(
        SET_COOKIE,
        session_cookie("oauth_state", "", 0, secure)
            .parse()
            .expect("generated cookie is valid"),
    );

    Ok(response)
}

#[derive(Deserialize)]
pub struct LogoutForm {
    csrf: String,
}

pub async fn logout(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    axum::Form(form): axum::Form<LogoutForm>,
) -> Result<Response> {
    session.verify_csrf(&form.csrf)?;
    state
        .database
        .delete_session(&session.token_hash, session.github_id)
        .await?;

    let configuration = state.database.configuration().await?;
    let secure = configuration.admin_base_url.starts_with("https:");
    let mut response = Redirect::to("/login").into_response();

    response.headers_mut().append(
        SET_COOKIE,
        session_cookie("session", "", 0, secure)
            .parse()
            .expect("generated cookie is valid"),
    );

    Ok(response)
}

fn session_cookie(name: &str, value: &str, max_age_seconds: i64, secure: bool) -> String {
    let secure_attribute = if secure { "; Secure" } else { "" };

    format!(
        "{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age_seconds}{secure_attribute}"
    )
}

pub(super) fn validated_return_to(target: &str) -> Option<&str> {
    if target.len() > 2048
        || !target.starts_with('/')
        || target.starts_with("//")
        || target.contains(['\\', '#'])
        || target.chars().any(char::is_control)
    {
        return None;
    }

    let uri = target.parse::<Uri>().ok()?;
    if uri.scheme().is_some()
        || uri.authority().is_some()
        || uri.path_and_query()?.as_str() != target
    {
        return None;
    }

    let origin = reqwest::Url::parse("https://reports.invalid/").expect("static URL is valid");
    if origin.join(target).ok()?.origin() != origin.origin() {
        return None;
    }

    Some(target)
}

pub(super) fn url_with_next(path: &str, next: &str) -> String {
    let base = format!("https://reports.invalid{path}");
    let url = reqwest::Url::parse_with_params(&base, &[("next", next)])
        .expect("static internal path is valid");
    format!("{path}?{}", url.query().expect("next parameter exists"))
}

#[cfg(test)]
mod tests {
    use super::{url_with_next, validated_return_to};

    #[test]
    fn return_destination_must_be_a_same_site_path() {
        for valid in ["/", "/reports/123", "/reports/123?source=discord&view=raw"] {
            assert_eq!(validated_return_to(valid), Some(valid));
        }

        for invalid in [
            "https://example.com/",
            "//example.com/",
            "/\\example.com/",
            "/reports/1#fragment",
            "/reports/1\r\nLocation: https://example.com",
        ] {
            assert_eq!(validated_return_to(invalid), None);
        }
    }

    #[test]
    fn login_link_preserves_path_and_query() {
        let link = url_with_next("/auth/github", "/reports/123?source=discord&view=raw");
        let url = reqwest::Url::parse(&format!("https://reports.invalid{link}"))
            .expect("internal login link is valid");
        assert_eq!(
            url.query_pairs().find(|(key, _)| key == "next").unwrap().1,
            "/reports/123?source=discord&view=raw"
        );
    }
}
