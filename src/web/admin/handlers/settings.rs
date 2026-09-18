use askama::Template;
use axum::{Extension, Form, extract::State, http::StatusCode, response::Redirect};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::{
    domain::{FieldKind, RuntimeConfiguration, SETTING_DEFINITIONS},
    error::{AppError, Result},
};

use super::super::{AdminState, TemplateResponse, authentication::Navigation, session::Session};

#[derive(Template)]
#[template(path = "settings/show.html")]
pub struct SettingsTemplate {
    navigation: Option<Navigation>,
    asset_version: &'static str,
    configuration: String,
    updated_at: DateTime<Utc>,
    fields: Vec<FieldView>,
    setting_definitions: Vec<SettingDefinitionView>,
}

pub struct FieldView {
    key: String,
    label: String,
    kind: String,
}

pub struct SettingDefinitionView {
    key: &'static str,
    path: &'static str,
    title: &'static str,
    description: &'static str,
    value_description: &'static str,
}

#[derive(Template)]
#[template(path = "settings/operations.html")]
pub struct OperationsTemplate {
    navigation: Option<Navigation>,
    asset_version: &'static str,
    events: Vec<EventView>,
}

pub struct EventView {
    action: String,
    actor: String,
    target_label: Option<String>,
    target_url: Option<String>,
    details: String,
    created_at: String,
}

pub async fn show(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
) -> Result<TemplateResponse<SettingsTemplate>> {
    let configuration = state.database.configuration_record().await?;
    let fields = state
        .database
        .field_definitions()
        .await?
        .into_iter()
        .map(|field| FieldView {
            key: field.key,
            label: field.label,
            kind: field.kind,
        })
        .collect();

    let configuration_json = serde_json::to_string_pretty(&configuration.value)
        .map_err(|error| AppError::Internal(error.into()))?;

    Ok(TemplateResponse(SettingsTemplate {
        navigation: Some(Navigation::for_session(&state, &session)),
        asset_version: super::assets::asset_version(),
        configuration: configuration_json,
        updated_at: configuration.updated_at,
        fields,
        setting_definitions: SETTING_DEFINITIONS
            .iter()
            .map(|definition| SettingDefinitionView {
                key: definition.key,
                path: definition.path,
                title: definition.title,
                description: definition.description,
                value_description: definition.value_description,
            })
            .collect(),
    }))
}

#[derive(Deserialize)]
pub struct ConfigurationForm {
    csrf: String,
    configuration: String,
}

pub async fn update(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Form(form): Form<ConfigurationForm>,
) -> Result<Redirect> {
    session.verify_csrf(&form.csrf)?;

    let configuration: RuntimeConfiguration = serde_json::from_str(&form.configuration)
        .map_err(|_| AppError::InvalidRequest("Invalid configuration JSON"))?;

    state
        .database
        .update_configuration(&configuration, session.github_id)
        .await?;

    tracing::info!(event = "configuration.update", actor = session.login,);

    Ok(Redirect::to("/settings"))
}

#[derive(Deserialize)]
pub struct FieldDefinitionForm {
    csrf: String,
    key: String,
    label: String,
    kind: String,
}

pub async fn update_field(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Form(form): Form<FieldDefinitionForm>,
) -> Result<Redirect> {
    session.verify_csrf(&form.csrf)?;

    let kind =
        FieldKind::parse(&form.kind).ok_or(AppError::InvalidRequest("Invalid field type"))?;
    state
        .database
        .upsert_field_definition(&form.key, &form.label, &kind, session.github_id)
        .await?;

    tracing::info!(
        event = "field_definition.update",
        field_key = form.key,
        actor = session.login,
    );

    Ok(Redirect::to("/settings"))
}

#[derive(Deserialize)]
pub struct FieldOrderForm {
    csrf: String,
    keys: String,
}

pub async fn reorder_fields(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
    Form(form): Form<FieldOrderForm>,
) -> Result<StatusCode> {
    session.verify_csrf(&form.csrf)?;

    let keys = form
        .keys
        .split(',')
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();

    state
        .database
        .reorder_field_definitions(&keys, session.github_id)
        .await?;

    tracing::info!(
        event = "field_definitions.reorder",
        field_count = keys.len(),
        actor = session.login,
    );

    Ok(StatusCode::NO_CONTENT)
}

pub async fn operations(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
) -> Result<TemplateResponse<OperationsTemplate>> {
    let events = state
        .database
        .recent_operations()
        .await?
        .into_iter()
        .map(|event| {
            let target = event.entity_id.map(|id| {
                let identifier = id.to_string();
                let suffix = &identifier[identifier.len() - 8..];
                let is_issue = event.action.starts_with("issue.");
                let label = if is_issue {
                    format!("Issue ·{suffix}")
                } else {
                    format!("Report ·{suffix}")
                };
                let url = if event.action == "issue.update_visibility" {
                    None
                } else if is_issue {
                    Some(format!("/issues/{id}"))
                } else {
                    Some(format!("/reports/{id}"))
                };
                (label, url)
            });

            EventView {
                action: event.action,
                actor: event.actor_login.unwrap_or_else(|| "system".into()),
                target_label: target.as_ref().map(|(label, _)| label.clone()),
                target_url: target.and_then(|(_, url)| url),
                details: event.details.to_string(),
                created_at: event.created_at.format("%d %b %Y, %H:%M UTC").to_string(),
            }
        })
        .collect();

    Ok(TemplateResponse(OperationsTemplate {
        navigation: Some(Navigation::for_session(&state, &session)),
        asset_version: super::assets::asset_version(),
        events,
    }))
}
