use std::collections::{BTreeSet, HashSet};

use serde_json::Value;
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
            "SELECT value, updated_at
             FROM runtime_configuration
             WHERE singleton = true",
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(ConfigurationRecord {
            value: row.get("value"),
            updated_at: row.get("updated_at"),
        })
    }

    pub async fn update_configuration(
        &self,
        configuration: &RuntimeConfiguration,
        actor: i64,
    ) -> Result<()> {
        configuration.validate()?;

        let value = serde_json::to_value(configuration)
            .map_err(|error| AppError::Internal(error.into()))?;
        let mut transaction = self.pool.begin().await?;
        let previous: Value = sqlx::query_scalar(
            "SELECT value FROM runtime_configuration WHERE singleton = true FOR UPDATE",
        )
        .fetch_one(&mut *transaction)
        .await?;
        let changed = changed_configuration_paths(&previous, &value);

        if changed.is_empty() {
            transaction.commit().await?;
            return Ok(());
        }

        sqlx::query(
            "UPDATE runtime_configuration
             SET value = $1, updated_at = now()
             WHERE singleton = true",
        )
        .bind(value)
        .execute(&mut *transaction)
        .await?;

        if changed
            .iter()
            .any(|path| path == "github_authorization_team")
        {
            // An authorization policy change must apply to sessions that were
            // verified under the old team, including the current session.
            sqlx::query("DELETE FROM sessions")
                .execute(&mut *transaction)
                .await?;
        }

        sqlx::query(
            "INSERT INTO audit_events (actor, action, details)
             VALUES ($1, 'configuration.update', $2)",
        )
        .bind(actor)
        .bind(serde_json::json!({ "changed": changed }))
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(())
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

        let previous =
            sqlx::query("SELECT label, kind FROM field_definitions WHERE key = $1 FOR UPDATE")
                .bind(key)
                .fetch_optional(&mut *transaction)
                .await?;
        let old = previous.as_ref().map(|row| {
            serde_json::json!({
                "label": row.get::<String, _>("label"),
                "kind": row.get::<String, _>("kind"),
            })
        });
        let new = serde_json::json!({ "label": label, "kind": kind.as_str() });

        if old.as_ref() == Some(&new) {
            transaction.commit().await?;
            return Ok(());
        }

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
             VALUES ($1, $2, $3)",
        )
        .bind(actor)
        .bind(if old.is_some() {
            "field_definition.update"
        } else {
            "field_definition.create"
        })
        .bind(serde_json::json!({ "key": key, "from": old, "to": new }))
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

        let existing_order = sqlx::query_scalar::<_, String>(
            "SELECT key FROM field_definitions ORDER BY position, key FOR UPDATE",
        )
        .fetch_all(&mut *transaction)
        .await?;
        let existing_keys = existing_order.iter().collect::<HashSet<_>>();

        if unique_keys != existing_keys {
            return Err(AppError::Conflict(
                "Field definitions changed while they were being reordered",
            ));
        }

        if keys == existing_order {
            transaction.commit().await?;
            return Ok(());
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
             VALUES ($1, 'field_definitions.reorder', $2)",
        )
        .bind(actor)
        .bind(serde_json::json!({
            "from": existing_order,
            "to": keys,
        }))
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
                audit_events.entity_id,
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
                entity_id: row.get("entity_id"),
                details: row.get("details"),
                created_at: row.get("created_at"),
            })
            .collect())
    }
}

fn changed_configuration_paths(previous: &Value, current: &Value) -> Vec<String> {
    fn collect(previous: &Value, current: &Value, prefix: &str, changed: &mut Vec<String>) {
        match (previous, current) {
            (Value::Object(before), Value::Object(after)) => {
                let keys = before.keys().chain(after.keys()).collect::<BTreeSet<_>>();
                for key in keys {
                    let path = if prefix.is_empty() {
                        key.to_owned()
                    } else {
                        format!("{prefix}.{key}")
                    };
                    match (before.get(key), after.get(key)) {
                        (Some(old), Some(new)) => collect(old, new, &path, changed),
                        _ => changed.push(path),
                    }
                }
            }
            _ if previous != current => changed.push(prefix.to_owned()),
            _ => {}
        }
    }

    let mut changed = Vec::new();
    collect(previous, current, "", &mut changed);
    changed
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::changed_configuration_paths;

    #[test]
    fn configuration_audit_records_paths_without_values() {
        let previous = json!({
            "limits": { "report_count": 10 },
            "discord_webhook_url": "old-secret",
        });
        let current = json!({
            "limits": { "report_count": 20 },
            "discord_webhook_url": "new-secret",
        });

        assert_eq!(
            changed_configuration_paths(&previous, &current),
            ["discord_webhook_url", "limits.report_count"]
        );
    }
}
