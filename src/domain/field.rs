use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Number;

use crate::{
    domain::{AttachmentId, IngestionLimits},
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
    Number(Number),
    Boolean(bool),
    Attachment(AttachmentId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FieldKind {
    Text,
    Multiline,
    StackTrace,
    Number,
    Boolean,
    Attachment,
}

#[derive(Clone, Debug)]
pub struct FieldDefinition {
    pub key: String,
    pub label: String,
    pub kind: FieldKind,
    pub position: i32,
}

impl FieldKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Multiline => "multiline",
            Self::StackTrace => "stack_trace",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::Attachment => "attachment",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "text" => Some(Self::Text),
            "multiline" => Some(Self::Multiline),
            "stack_trace" => Some(Self::StackTrace),
            "number" => Some(Self::Number),
            "boolean" => Some(Self::Boolean),
            "attachment" => Some(Self::Attachment),
            _ => None,
        }
    }
}

impl FieldValue {
    pub fn kind(&self) -> FieldKind {
        match self {
            Self::Text(_) => FieldKind::Text,
            Self::Multiline(_) => FieldKind::Multiline,
            Self::StackTrace(_) => FieldKind::StackTrace,
            Self::Number(_) => FieldKind::Number,
            Self::Boolean(_) => FieldKind::Boolean,
            Self::Attachment(_) => FieldKind::Attachment,
        }
    }

    pub fn json_value(&self) -> serde_json::Value {
        match self {
            Self::Text(value) | Self::Multiline(value) | Self::StackTrace(value) => {
                value.clone().into()
            }
            Self::Number(value) => value.clone().into(),
            Self::Boolean(value) => (*value).into(),
            Self::Attachment(value) => value.to_string().into(),
        }
    }
}

pub fn validate_fields(
    fields: &[DiagnosticField],
    attachment_ids: &HashSet<AttachmentId>,
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
            Some(definition)
                if definition.kind != field.value.kind()
                    && !(definition.kind == FieldKind::StackTrace
                        && field.value.kind() == FieldKind::Multiline) =>
            {
                return Err(AppError::InvalidRequest(
                    "Recognized diagnostic field has the wrong type",
                ));
            }
            _ => {}
        }

        match &field.value {
            FieldValue::Text(value) if value.len() > limits.short_text_bytes => {
                return Err(AppError::InvalidRequest("Text field is too large"));
            }
            FieldValue::Multiline(value) | FieldValue::StackTrace(value)
                if value.len() > limits.multiline_text_bytes =>
            {
                return Err(AppError::InvalidRequest("Multiline field is too large"));
            }
            FieldValue::Attachment(id) if !attachment_ids.contains(id) => {
                return Err(AppError::InvalidRequest(
                    "Diagnostic field references an unknown attachment",
                ));
            }
            _ => {}
        }
    }

    Ok(())
}

fn validate_field_key(key: &str) -> Result<()> {
    let valid = !key.is_empty()
        && key.len() <= 64
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte));

    if !valid {
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
                label: "Native stack".into(),
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
}
