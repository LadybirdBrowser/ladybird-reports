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
    events: Vec<EventView>,
}

pub struct EventView {
    action: String,
    actor: String,
    details: String,
    created_at: DateTime<Utc>,
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

    tracing::info!(event = "configuration.updated", actor = session.login,);

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
        event = "field_definition.updated",
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
        event = "field_definitions.reordered",
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
        .map(|event| EventView {
            action: event.action,
            actor: event.actor_login.unwrap_or_else(|| "system".into()),
            details: event.details.to_string(),
            created_at: event.created_at,
        })
        .collect();

    Ok(TemplateResponse(OperationsTemplate {
        navigation: Some(Navigation::for_session(&state, &session)),
        events,
    }))
}
