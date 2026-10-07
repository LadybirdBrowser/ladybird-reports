use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Number;

use crate::{
    domain::{AttachmentReference, IngestionLimits},
    error::{AppError, Result},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticField {
    pub key: String,
    pub value: FieldValue,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum FieldValue {
    Text(String),
    Multiline(String),
    StackTrace(String),
    Url(String),
    CommitId(String),
    Number(Number),
    Boolean(bool),
    Attachment(AttachmentReference),
}

text_enum! {
    pub enum FieldKind {
        Text => "text",
        Multiline => "multiline",
        StackTrace => "stack_trace",
        Url => "url",
        CommitId => "commit_id",
        Number => "number",
        Boolean => "boolean",
        Attachment => "attachment",
    }
}

impl FieldKind {
    /// Whether a field defined with this kind may be submitted as a value of
    /// `submitted`. Older clients send stack traces as multiline text, and URLs
    /// and commit IDs as plain text.
    pub fn accepts(self, submitted: FieldKind) -> bool {
        self == submitted
            || matches!(
                (self, submitted),
                (FieldKind::StackTrace, FieldKind::Multiline)
                    | (FieldKind::Url | FieldKind::CommitId, FieldKind::Text)
            )
    }
}

/// Whether the text is a plain hexadecimal commit ID, abbreviated or in full.
/// Nothing else is allowed, so it is safe to place in an address.
pub fn is_commit_id(value: &str) -> bool {
    (7..=64).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Clone, Debug)]
pub struct FieldDefinition {
    pub key: String,
    pub label: String,
    pub kind: FieldKind,
    pub position: i32,
}

impl FieldValue {
    pub fn kind(&self) -> FieldKind {
        match self {
            Self::Text(_) => FieldKind::Text,
            Self::Multiline(_) => FieldKind::Multiline,
            Self::StackTrace(_) => FieldKind::StackTrace,
            Self::Url(_) => FieldKind::Url,
            Self::CommitId(_) => FieldKind::CommitId,
            Self::Number(_) => FieldKind::Number,
            Self::Boolean(_) => FieldKind::Boolean,
            Self::Attachment(_) => FieldKind::Attachment,
        }
    }

    pub fn json_value(&self) -> serde_json::Value {
        match self {
            Self::Text(value)
            | Self::Multiline(value)
            | Self::StackTrace(value)
            | Self::Url(value)
            | Self::CommitId(value) => value.clone().into(),
            Self::Number(value) => value.clone().into(),
            Self::Boolean(value) => (*value).into(),
            Self::Attachment(value) => value.as_str().into(),
        }
    }
}

pub fn validate_fields(
    fields: &[DiagnosticField],
    attachment_references: &HashSet<&AttachmentReference>,
    definitions: &HashMap<String, FieldDefinition>,
    limits: &IngestionLimits,
) -> Result<()> {
    if fields.len() > limits.maximum_fields {
        return Err(AppError::InvalidRequest("Too many diagnostic fields"));
    }

    let mut keys = HashSet::new();

    for field in fields {
        validate_field_key(&field.key)?;

        if !keys.insert(&field.key) {
            return Err(AppError::InvalidRequest("Duplicate diagnostic field"));
        }

        match definitions.get(&field.key) {
            Some(definition) if !definition.kind.accepts(field.value.kind()) => {
                return Err(AppError::InvalidRequest(
                    "Recognized diagnostic field has the wrong type",
                ));
            }
            _ => {}
        }

        match &field.value {
            FieldValue::Text(value) | FieldValue::Url(value) | FieldValue::CommitId(value)
                if value.len() > limits.short_text_bytes =>
            {
                return Err(AppError::InvalidRequest("Text field is too large"));
            }
            FieldValue::Multiline(value) | FieldValue::StackTrace(value)
                if value.len() > limits.multiline_text_bytes =>
            {
                return Err(AppError::InvalidRequest("Multiline field is too large"));
            }
            FieldValue::Attachment(reference) if !attachment_references.contains(reference) => {
                return Err(AppError::InvalidRequest(
                    "Diagnostic field references an unknown attachment",
                ));
            }
            _ => {}
        }
    }

    Ok(())
}

/// The shape of a diagnostic field key, shared by submissions and field definitions.
pub fn is_valid_field_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

fn validate_field_key(key: &str) -> Result<()> {
    if !is_valid_field_key(key) {
        return Err(AppError::InvalidRequest("Invalid diagnostic field key"));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_trace_fields_accept_new_and_legacy_text_envelopes() {
        let definitions = HashMap::from([(
            "stack".to_owned(),
            FieldDefinition {
                key: "stack".into(),
                label: "Stack trace".into(),
                kind: FieldKind::StackTrace,
                position: 0,
            },
        )]);
        let attachments = HashSet::new();
        let limits = IngestionLimits::default();

        for value in [
            FieldValue::StackTrace("#0 0x123 function".into()),
            FieldValue::Multiline("#0 0x123 function".into()),
        ] {
            let fields = [DiagnosticField {
                key: "stack".into(),
                value,
            }];
            assert!(validate_fields(&fields, &attachments, &definitions, &limits).is_ok());
        }

        let fields = [DiagnosticField {
            key: "stack".into(),
            value: FieldValue::Text("not a stack".into()),
        }];
        assert!(validate_fields(&fields, &attachments, &definitions, &limits).is_err());
    }

    fn definitions_for(key: &str, kind: FieldKind) -> HashMap<String, FieldDefinition> {
        HashMap::from([(
            key.to_owned(),
            FieldDefinition {
                key: key.into(),
                label: key.into(),
                kind,
                position: 0,
            },
        )])
    }

    fn validate_one(
        key: &str,
        value: FieldValue,
        definitions: &HashMap<String, FieldDefinition>,
    ) -> Result<()> {
        validate_fields(
            &[DiagnosticField {
                key: key.into(),
                value,
            }],
            &HashSet::new(),
            definitions,
            &IngestionLimits::default(),
        )
    }

    #[test]
    fn commit_ids_are_plain_hexadecimal_text() {
        assert!(is_commit_id("a71cffa"));
        assert!(is_commit_id("A71CFFAE5AD9D29729BB3364A49A637895286E1F"));

        for value in [
            "",
            "a71cff",
            "a71cffg",
            "a71cffa ",
            "a71cffa/../x",
            "a71cffa?x=1",
            "a71cffa\n",
            &"a".repeat(65),
        ] {
            assert!(!is_commit_id(value), "{value:?}");
        }
    }

    #[test]
    fn commit_id_definitions_accept_any_short_text() {
        let definitions = definitions_for("git_commit", FieldKind::CommitId);

        // Builds without git information report "unknown"; only a valid commit
        // ID is ever turned into a link.
        for value in [
            FieldValue::Text("a71cffae5ad9".into()),
            FieldValue::Text("unknown".into()),
            FieldValue::Text(String::new()),
            FieldValue::CommitId("unknown".into()),
        ] {
            assert!(validate_one("git_commit", value, &definitions).is_ok());
        }
    }

    #[test]
    fn url_definitions_accept_text_and_url_values() {
        let definitions = definitions_for("url", FieldKind::Url);

        for value in [
            FieldValue::Text("https://example.com/".into()),
            FieldValue::Url("https://example.com/".into()),
        ] {
            assert!(validate_one("url", value, &definitions).is_ok());
        }

        assert!(validate_one("url", FieldValue::Boolean(true), &definitions).is_err());
    }
}
