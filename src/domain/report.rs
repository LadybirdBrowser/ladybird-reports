use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::{
    domain::{
        AttachmentId, DiagnosticField, FieldDefinition, IngestionLimits, SubmissionId,
        validate_fields,
    },
    error::{AppError, Result},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReportManifest {
    pub protocol: u32,
    pub submission_id: SubmissionId,
    pub kind: ReportKind,
    pub client_version: String,
    pub build: String,
    pub fields: Vec<DiagnosticField>,
    pub attachments: Vec<AttachmentManifest>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportKind {
    Crash,
    WebCompat,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AttachmentManifest {
    pub id: AttachmentId,
    pub name: String,
    pub media_type: AttachmentMediaType,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum AttachmentMediaType {
    #[serde(rename = "image/png")]
    Png,

    #[serde(rename = "text/plain")]
    PlainText,
}

impl AttachmentMediaType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::PlainText => "text/plain",
        }
    }
}

impl ReportKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Crash => "crash",
            Self::WebCompat => "web_compat",
        }
    }
}

impl ReportManifest {
    pub fn validate(
        &self,
        definitions: &HashMap<String, FieldDefinition>,
        limits: &IngestionLimits,
    ) -> Result<()> {
        if self.protocol != 1 {
            return Err(AppError::InvalidRequest("Unsupported report protocol"));
        }

        if self.client_version.is_empty() || self.client_version.len() > 256 {
            return Err(AppError::InvalidRequest("Invalid client version"));
        }

        if self.build.len() > 1024 {
            return Err(AppError::InvalidRequest("Build description is too large"));
        }

        if self.attachments.len() > limits.maximum_attachments {
            return Err(AppError::InvalidRequest("Too many attachments"));
        }

        let attachment_ids = self.validate_attachments(limits)?;
        validate_fields(&self.fields, &attachment_ids, definitions, limits)
    }

    fn validate_attachments(&self, limits: &IngestionLimits) -> Result<HashSet<AttachmentId>> {
        let mut attachment_ids = HashSet::new();
        let mut total_size = 0_u64;

        for attachment in &self.attachments {
            if !attachment_ids.insert(attachment.id) {
                return Err(AppError::InvalidRequest("Duplicate attachment ID"));
            }

            validate_attachment_name(&attachment.name)?;

            if attachment.size > limits.attachment_bytes as u64 {
                return Err(AppError::InvalidRequest("Attachment is too large"));
            }

            if !is_sha256_hex(&attachment.sha256) {
                return Err(AppError::InvalidRequest("Invalid attachment digest"));
            }

            total_size = total_size
                .checked_add(attachment.size)
                .ok_or(AppError::InvalidRequest("Attachments are too large"))?;
        }

        if total_size > limits.submission_bytes as u64 {
            return Err(AppError::InvalidRequest("Attachments are too large"));
        }

        Ok(attachment_ids)
    }
}

pub fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_attachment_name(name: &str) -> Result<()> {
    let valid = !name.is_empty()
        && name.len() <= 128
        && !name
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\'));

    if !valid {
        return Err(AppError::InvalidRequest("Invalid attachment name"));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpeg_attachments_are_not_part_of_the_protocol() {
        let json = serde_json::json!({
            "id": AttachmentId::new(),
            "name": "screenshot.jpg",
            "media_type": "image/jpeg",
            "size": 4,
            "sha256": "a".repeat(64),
        });

        assert!(serde_json::from_value::<AttachmentManifest>(json).is_err());
    }

    #[test]
    fn unknown_envelope_members_require_a_new_protocol_version() {
        let json = serde_json::json!({
            "protocol": 1,
            "submission_id": SubmissionId::new(),
            "kind": "web_compat",
            "client_version": "test",
            "build": "Debug",
            "fields": [],
            "attachments": [],
            "future_member": true,
        });

        assert!(serde_json::from_value::<ReportManifest>(json).is_err());
    }

    #[test]
    fn web_compat_is_a_supported_report_kind() {
        let kind: ReportKind = serde_json::from_str("\"web_compat\"").expect("report kind");
        assert_eq!(kind.as_str(), "web_compat");
    }
}
