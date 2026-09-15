use std::collections::HashSet;

use sqlx::Row;

use crate::{
    domain::{FieldKind, RuntimeConfiguration},
    error::{AppError, Result},
    infrastructure::database::AdminDatabase,
};

use super::{ConfigurationRecord, FieldDefinitionRecord};

impl AdminDatabase {
    pub async fn configuration(&self) -> Result<RuntimeConfiguration> {
        let record = self.configuration_record().await?;
        let configuration: RuntimeConfiguration = serde_json::from_value(record.value)
            .map_err(|error| AppError::Internal(error.into()))?;

        configuration.validate()?;
        Ok(configuration)
    }

    pub async fn configuration_record(&self) -> Result<ConfigurationRecord> {
        let row = sqlx::query(
            "SELECT value, revision, updated_at
             FROM runtime_configuration
             WHERE singleton = true",
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(ConfigurationRecord {
            value: row.get("value"),
            revision: row.get("revision"),
            updated_at: row.get("updated_at"),
        })
    }

    pub async fn update_configuration(
        &self,
        configuration: &RuntimeConfiguration,
        expected_revision: i64,
        actor: i64,
    ) -> Result<i64> {
        configuration.validate()?;

        let value = serde_json::to_value(configuration)
            .map_err(|error| AppError::Internal(error.into()))?;
        let mut transaction = self.pool.begin().await?;

        let row = sqlx::query(
            "UPDATE runtime_configuration
             SET value = $1, revision = revision + 1, updated_at = now()
             WHERE singleton = true AND revision = $2
             RETURNING revision",
        )
        .bind(value)
        .bind(expected_revision)
        .fetch_optional(&mut *transaction)
        .await?;

        let Some(row) = row else {
            return Err(AppError::Conflict(
                "Configuration changed while it was being edited",
            ));
        };

        let revision: i64 = row.get("revision");

        sqlx::query(
            "INSERT INTO audit_events (actor, action, details)
             VALUES ($1, 'configuration.updated', $2)",
        )
        .bind(actor)
        .bind(serde_json::json!({ "revision": revision }))
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(revision)
    }

    pub async fn field_definitions(&self) -> Result<Vec<FieldDefinitionRecord>> {
        let rows = sqlx::query(
            "SELECT key, label, kind, position
             FROM field_definitions
             ORDER BY position, key",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| FieldDefinitionRecord {
                key: row.get("key"),
                label: row.get("label"),
                kind: row.get("kind"),
                position: row.get("position"),
            })
            .collect())
    }

    pub async fn upsert_field_definition(
        &self,
        key: &str,
        label: &str,
        kind: &FieldKind,
        actor: i64,
    ) -> Result<()> {
        validate_field_definition(key, label)?;

        let mut transaction = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(891125)")
            .execute(&mut *transaction)
            .await?;

        sqlx::query(
            "INSERT INTO field_definitions (key, label, kind, position)
             SELECT $1, $2, $3, COALESCE(MAX(position) + 10, 0)
             FROM field_definitions
             ON CONFLICT (key) DO UPDATE SET
                label = excluded.label,
                kind = excluded.kind",
        )
        .bind(key)
        .bind(label)
        .bind(kind.as_str())
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            "INSERT INTO audit_events (actor, action, details)
             VALUES ($1, 'field_definition.updated', $2)",
        )
        .bind(actor)
        .bind(serde_json::json!({ "key": key }))
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(())
    }

    pub async fn reorder_field_definitions(&self, keys: &[String], actor: i64) -> Result<()> {
        if keys.is_empty() || keys.len() > 256 {
            return Err(AppError::InvalidRequest("Invalid field order"));
        }

        for key in keys {
            validate_field_key(key)?;
        }

        let unique_keys = keys.iter().collect::<HashSet<_>>();
        if unique_keys.len() != keys.len() {
            return Err(AppError::InvalidRequest("Field order contains duplicates"));
        }

        let mut transaction = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(891125)")
            .execute(&mut *transaction)
            .await?;

        let existing_keys = sqlx::query_scalar::<_, String>(
            "SELECT key FROM field_definitions ORDER BY key FOR UPDATE",
        )
        .fetch_all(&mut *transaction)
        .await?;
        let existing_keys = existing_keys.iter().collect::<HashSet<_>>();

        if unique_keys != existing_keys {
            return Err(AppError::Conflict(
                "Field definitions changed while they were being reordered",
            ));
        }

        sqlx::query(
            "UPDATE field_definitions
             SET position = ordered.position
             FROM (
                SELECT key, ((ordinality - 1) * 10)::integer AS position
                FROM unnest($1::text[]) WITH ORDINALITY AS fields(key, ordinality)
             ) AS ordered
             WHERE field_definitions.key = ordered.key",
        )
        .bind(keys)
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            "INSERT INTO audit_events (actor, action, details)
             VALUES ($1, 'field_definitions.reordered', $2)",
        )
        .bind(actor)
        .bind(serde_json::json!({ "keys": keys }))
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(())
    }

    pub async fn recent_operations(&self) -> Result<Vec<super::AuditEvent>> {
        let rows = sqlx::query(
            "SELECT
                audit_events.action,
                maintainers.login AS actor_login,
                audit_events.details,
                audit_events.created_at
             FROM audit_events
             LEFT JOIN maintainers ON maintainers.github_id = audit_events.actor
             ORDER BY audit_events.id DESC
             LIMIT 200",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| super::AuditEvent {
                action: row.get("action"),
                actor_login: row.get("actor_login"),
                details: row.get("details"),
                created_at: row.get("created_at"),
            })
            .collect())
    }
}

fn validate_field_definition(key: &str, label: &str) -> Result<()> {
    validate_field_key(key)?;

    if label.is_empty() || label.len() > 128 {
        return Err(AppError::InvalidRequest("Invalid field label"));
    }

    Ok(())
}

fn validate_field_key(key: &str) -> Result<()> {
    let valid_key = !key.is_empty()
        && key.len() <= 64
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte));

    if !valid_key {
        return Err(AppError::InvalidRequest("Invalid field key"));
    }

    Ok(())
}
