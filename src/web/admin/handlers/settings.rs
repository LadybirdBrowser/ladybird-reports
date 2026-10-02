use askama::Template;
use axum::{
    Extension, Form,
    extract::{Query, State},
    http::StatusCode,
    response::Redirect,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::{
    domain::{AuditAction, AuditEntity, FieldKind, RuntimeConfiguration, SETTING_DEFINITIONS},
    error::{AppError, Result},
    infrastructure::database::AuditEvent,
};

use super::super::{
    AdminState, TemplateResponse, authentication::Navigation, session::Session,
    templates::display_timestamp,
};

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
    kind: FieldKind,
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
    next_page: Option<String>,
}

#[derive(Template)]
#[template(path = "settings/_operations_list.html")]
pub struct OperationsListTemplate {
    events: Vec<EventView>,
    next_page: Option<String>,
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

    let configuration_json =
        serde_json::to_string_pretty(&configuration.value).map_err(AppError::internal)?;

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

const OPERATIONS_PAGE_SIZE: usize = 100;

#[derive(Deserialize)]
pub struct OperationsQuery {
    before_id: Option<i64>,
}

pub async fn operations(
    State(state): State<AdminState>,
    Extension(session): Extension<Session>,
) -> Result<TemplateResponse<OperationsTemplate>> {
    let list = load_operations(&state, None).await?;

    Ok(TemplateResponse(OperationsTemplate {
        navigation: Some(Navigation::for_session(&state, &session)),
        asset_version: super::assets::asset_version(),
        events: list.events,
        next_page: list.next_page,
    }))
}

pub async fn operations_list(
    State(state): State<AdminState>,
    Query(query): Query<OperationsQuery>,
) -> Result<TemplateResponse<OperationsListTemplate>> {
    Ok(TemplateResponse(
        load_operations(&state, query.before_id).await?,
    ))
}

async fn load_operations(
    state: &AdminState,
    before_id: Option<i64>,
) -> Result<OperationsListTemplate> {
    // One extra row tells us whether another page exists without a count query.
    let mut events = state
        .database
        .operations_page(before_id, OPERATIONS_PAGE_SIZE as i64 + 1)
        .await?;
    let has_more = events.len() > OPERATIONS_PAGE_SIZE;
    events.truncate(OPERATIONS_PAGE_SIZE);

    let next_page = events
        .last()
        .filter(|_| has_more)
        .map(|last| format!("/api/operations-list?before_id={}", last.id));

    Ok(OperationsListTemplate {
        events: events.into_iter().map(event_view).collect(),
        next_page,
    })
}

fn event_view(event: AuditEvent) -> EventView {
    // Rows written by earlier releases may hold actions that no longer exist.
    let action = AuditAction::parse(&event.action);
    let target = event.entity_id.map(|id| {
        let identifier = id.to_string();
        let suffix = &identifier[identifier.len() - 8..];
        let (label, section) = match AuditEntity::of_stored_action(&event.action) {
            AuditEntity::Issue => (format!("Issue ·{suffix}"), "issues"),
            AuditEntity::Report => (format!("Report ·{suffix}"), "reports"),
        };
        let has_page = action.is_none_or(AuditAction::entity_has_page);

        (label, has_page.then(|| format!("/{section}/{id}")))
    });

    let actor = event
        .actor_login
        .or_else(|| denied_sign_in_login(action, &event.details))
        .unwrap_or_else(|| "system".into());

    EventView {
        action: event.action,
        actor,
        target_label: target.as_ref().map(|(label, _)| label.clone()),
        target_url: target.and_then(|(_, url)| url),
        details: event.details.to_string(),
        created_at: display_timestamp(event.created_at),
    }
}

/// A denied sign-in has no maintainer actor, so the account that tried to sign
/// in is shown instead of "system".
fn denied_sign_in_login(
    action: Option<AuditAction>,
    details: &serde_json::Value,
) -> Option<String> {
    if action != Some(AuditAction::SessionDenied) {
        return None;
    }

    details.get("login")?.as_str().map(str::to_owned)
}
