use ipnet::IpNet;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

pub const HARD_MAX_BODY_BYTES: usize = 66 * 1024 * 1024;
pub const HARD_MAX_FIELDS: usize = 256;
pub const HARD_MAX_ATTACHMENTS: usize = 16;
pub const HARD_MAX_PNG_PIXELS: u64 = 50_000_000;
pub const HARD_MAX_PNG_DECODED_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfiguration {
    pub public_base_url: String,
    pub admin_base_url: String,
    pub github_repository: String,
    pub trusted_proxies: Vec<IpNet>,
    pub limits: IngestionLimits,
    pub proof_of_work: ProofOfWorkConfiguration,
    pub maintenance: MaintenanceConfiguration,
    pub membership_recheck_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IngestionLimits {
    pub public_requests_per_minute: u32,
    pub public_request_burst: u32,
    pub challenges_per_minute: u32,
    pub challenge_burst: u32,
    pub challenges_per_hour: u32,
    pub submissions_per_minute: u32,
    pub submission_burst: u32,
    pub submissions_per_hour: u32,
    pub concurrent_uploads_per_ip: u32,
    pub concurrent_uploads_global: u32,
    pub maximum_fields: usize,
    pub short_text_bytes: usize,
    pub multiline_text_bytes: usize,
    pub metadata_bytes: usize,
    pub maximum_attachments: usize,
    pub attachment_bytes: usize,
    pub submission_bytes: usize,
    pub png_pixels: u64,
    pub png_decoded_bytes: usize,
    pub upload_timeout_seconds: u64,
    pub minimum_free_storage_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProofOfWorkConfiguration {
    pub expected_work: u64,
    pub challenge_lifetime_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceConfiguration {
    pub sweep_interval_seconds: u64,
    pub staging_retention_seconds: u64,
    pub report_retention_days: u32,
}

impl Default for RuntimeConfiguration {
    fn default() -> Self {
        Self {
            public_base_url: "https://reports.app.ladybird.org".into(),
            admin_base_url: "http://localhost:3000".into(),
            github_repository: "LadybirdBrowser/ladybird".into(),
            trusted_proxies: Vec::new(),
            limits: IngestionLimits::default(),
            proof_of_work: ProofOfWorkConfiguration {
                expected_work: 5_244_236,
                challenge_lifetime_seconds: 600,
            },
            maintenance: MaintenanceConfiguration {
                sweep_interval_seconds: 900,
                staging_retention_seconds: 3_600,
                report_retention_days: 3_650,
            },
            membership_recheck_seconds: 300,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SettingDefinition {
    pub key: &'static str,
    pub path: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub value_description: &'static str,
}

pub const SETTING_DEFINITIONS: &[SettingDefinition] = &[
    setting(
        "public_base_url",
        "public_base_url",
        "Public base URL",
        "The externally reachable origin used by reporting clients.",
        "An HTTPS origin without a path, query, or fragment.",
    ),
    setting(
        "admin_base_url",
        "admin_base_url",
        "Admin base URL",
        "The externally reachable origin used for OAuth callbacks and secure cookies.",
        "An HTTPS origin without a path, query, or fragment.",
    ),
    setting(
        "github_repository",
        "github_repository",
        "GitHub repository",
        "The repository searched and updated when an internal issue is linked to GitHub.",
        "An owner and repository name, such as LadybirdBrowser/ladybird.",
    ),
    setting(
        "trusted_proxies",
        "trusted_proxies",
        "Trusted proxies",
        "Proxy networks whose forwarding headers may identify the original client address.",
        "A JSON list of IPv4 or IPv6 CIDR ranges.",
    ),
    setting(
        "public_requests_per_minute",
        "limits.public_requests_per_minute",
        "Public request rate",
        "Refills the shared per-address request bucket used by every public API request.",
        "Requests added to the bucket per minute.",
    ),
    setting(
        "public_request_burst",
        "limits.public_request_burst",
        "Public request burst",
        "Caps short bursts across all public API endpoints from one address.",
        "Maximum immediately available requests.",
    ),
    setting(
        "challenges_per_minute",
        "limits.challenges_per_minute",
        "Challenge rate",
        "Refills the short-term proof-of-work challenge bucket for one address.",
        "Challenges added per minute.",
    ),
    setting(
        "challenge_burst",
        "limits.challenge_burst",
        "Challenge burst",
        "Caps how many proof-of-work challenges one address can request immediately.",
        "Maximum immediately available challenges.",
    ),
    setting(
        "challenges_per_hour",
        "limits.challenges_per_hour",
        "Hourly challenge limit",
        "Limits sustained proof-of-work challenge creation by one address.",
        "Maximum challenges in the hourly bucket.",
    ),
    setting(
        "submissions_per_minute",
        "limits.submissions_per_minute",
        "Submission rate",
        "Refills the short-term completed report submission bucket for one address.",
        "Submissions added per minute.",
    ),
    setting(
        "submission_burst",
        "limits.submission_burst",
        "Submission burst",
        "Caps how many reports one address can submit immediately.",
        "Maximum immediately available submissions.",
    ),
    setting(
        "submissions_per_hour",
        "limits.submissions_per_hour",
        "Hourly submission limit",
        "Limits sustained report submissions by one address.",
        "Maximum submissions in the hourly bucket.",
    ),
    setting(
        "concurrent_uploads_per_ip",
        "limits.concurrent_uploads_per_ip",
        "Concurrent uploads per address",
        "Limits simultaneous attachment uploads from one source address.",
        "Number of active uploads.",
    ),
    setting(
        "concurrent_uploads_global",
        "limits.concurrent_uploads_global",
        "Global concurrent uploads",
        "Caps simultaneous uploads across the service to bound memory and disk pressure.",
        "Number of active uploads across all clients.",
    ),
    setting(
        "maximum_fields",
        "limits.maximum_fields",
        "Maximum fields",
        "Limits the number of diagnostic fields accepted in one report manifest.",
        "Field count per report.",
    ),
    setting(
        "short_text_bytes",
        "limits.short_text_bytes",
        "Short text size",
        "Caps each text field classified as short text.",
        "Maximum UTF-8 bytes per value.",
    ),
    setting(
        "multiline_text_bytes",
        "limits.multiline_text_bytes",
        "Multiline text size",
        "Caps each multiline diagnostic such as a native stack trace.",
        "Maximum UTF-8 bytes per value.",
    ),
    setting(
        "metadata_bytes",
        "limits.metadata_bytes",
        "Manifest size",
        "Caps the JSON manifest before attachments are read.",
        "Maximum encoded bytes per manifest.",
    ),
    setting(
        "maximum_attachments",
        "limits.maximum_attachments",
        "Maximum attachments",
        "Limits the number of files accepted with one report.",
        "Attachment count per report.",
    ),
    setting(
        "attachment_bytes",
        "limits.attachment_bytes",
        "Attachment size",
        "Caps the compressed bytes accepted for each attachment.",
        "Maximum bytes per file.",
    ),
    setting(
        "submission_bytes",
        "limits.submission_bytes",
        "Submission size",
        "Caps the manifest and all attachment bytes in one request.",
        "Maximum total request payload bytes.",
    ),
    setting(
        "png_pixels",
        "limits.png_pixels",
        "PNG pixel count",
        "Rejects images whose width multiplied by height exceeds this limit.",
        "Maximum decoded pixel count.",
    ),
    setting(
        "png_decoded_bytes",
        "limits.png_decoded_bytes",
        "PNG decode memory",
        "Bounds decoded PNG frame memory and weights concurrent decoder permits.",
        "Maximum decoded bytes per image.",
    ),
    setting(
        "upload_timeout_seconds",
        "limits.upload_timeout_seconds",
        "Upload timeout",
        "Stops a report upload that does not complete within the configured window.",
        "Seconds from manifest acceptance through attachment upload.",
    ),
    setting(
        "minimum_free_storage_bytes",
        "limits.minimum_free_storage_bytes",
        "Storage reserve",
        "Rejects new uploads before the attachment volume consumes its reserved space.",
        "Bytes that must remain free after a maximum-size submission.",
    ),
    setting(
        "expected_work",
        "proof_of_work.expected_work",
        "Proof-of-work target",
        "Controls the average number of hashes required for a valid submission proof.",
        "Expected SHA-256 attempts; higher values require more client CPU time.",
    ),
    setting(
        "challenge_lifetime_seconds",
        "proof_of_work.challenge_lifetime_seconds",
        "Challenge lifetime",
        "Controls how long an issued proof-of-work challenge can be submitted.",
        "Seconds after challenge creation.",
    ),
    setting(
        "sweep_interval_seconds",
        "maintenance.sweep_interval_seconds",
        "Maintenance interval",
        "Controls how often both services remove expired and abandoned data.",
        "Seconds between maintenance sweeps.",
    ),
    setting(
        "staging_retention_seconds",
        "maintenance.staging_retention_seconds",
        "Staging retention",
        "Keeps interrupted upload directories long enough for active requests, then removes them.",
        "Seconds since the staging directory was last modified.",
    ),
    setting(
        "report_retention_days",
        "maintenance.report_retention_days",
        "Report retention",
        "Sets the expiry date assigned to new reports and their attachments.",
        "Days from report creation before deletion is scheduled.",
    ),
    setting(
        "membership_recheck_seconds",
        "membership_recheck_seconds",
        "Membership recheck",
        "Controls how often an active session revalidates GitHub team membership.",
        "Seconds between membership checks for one session.",
    ),
];

const fn setting(
    key: &'static str,
    path: &'static str,
    title: &'static str,
    description: &'static str,
    value_description: &'static str,
) -> SettingDefinition {
    SettingDefinition {
        key,
        path,
        title,
        description,
        value_description,
    }
}

impl Default for IngestionLimits {
    fn default() -> Self {
        Self {
            public_requests_per_minute: 120,
            public_request_burst: 30,
            challenges_per_minute: 10,
            challenge_burst: 3,
            challenges_per_hour: 100,
            submissions_per_minute: 5,
            submission_burst: 2,
            submissions_per_hour: 50,
            concurrent_uploads_per_ip: 2,
            concurrent_uploads_global: 16,
            maximum_fields: 64,
            short_text_bytes: 4 * 1024,
            multiline_text_bytes: 256 * 1024,
            metadata_bytes: 1024 * 1024,
            maximum_attachments: 5,
            attachment_bytes: 10 * 1024 * 1024,
            submission_bytes: 25 * 1024 * 1024,
            png_pixels: 25_000_000,
            png_decoded_bytes: 32 * 1024 * 1024,
            upload_timeout_seconds: 120,
            minimum_free_storage_bytes: 5 * 1024 * 1024 * 1024,
        }
    }
}

impl RuntimeConfiguration {
    pub fn validate(&self) -> Result<()> {
        self.validate_public_url()?;
        validate_origin(&self.admin_base_url, "Invalid admin URL")?;
        self.validate_github_repository()?;
        self.validate_rate_limits()?;
        self.validate_payload_limits()?;
        self.validate_operational_limits()?;

        Ok(())
    }

    fn validate_public_url(&self) -> Result<()> {
        validate_origin(&self.public_base_url, "Invalid public URL")
    }

    fn validate_github_repository(&self) -> Result<()> {
        let mut components = self.github_repository.split('/');
        let owner = components.next().unwrap_or_default();
        let repository = components.next().unwrap_or_default();

        let valid_component = |component: &str| {
            !component.is_empty()
                && component
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-._".contains(&byte))
        };

        if !valid_component(owner) || !valid_component(repository) || components.next().is_some() {
            return Err(AppError::InvalidRequest("Invalid GitHub repository"));
        }

        Ok(())
    }

    fn validate_rate_limits(&self) -> Result<()> {
        let limits = &self.limits;
        let values = [
            limits.public_requests_per_minute,
            limits.public_request_burst,
            limits.challenges_per_minute,
            limits.challenge_burst,
            limits.challenges_per_hour,
            limits.submissions_per_minute,
            limits.submission_burst,
            limits.submissions_per_hour,
        ];

        if values.iter().any(|value| !(1..=100_000).contains(value)) {
            return Err(AppError::InvalidRequest(
                "Rate limits must be between 1 and 100000",
            ));
        }

        Ok(())
    }

    fn validate_payload_limits(&self) -> Result<()> {
        let limits = &self.limits;

        let invalid = limits.maximum_fields == 0
            || limits.maximum_fields > HARD_MAX_FIELDS
            || limits.maximum_attachments > HARD_MAX_ATTACHMENTS
            || limits.metadata_bytes == 0
            || limits.metadata_bytes > 2 * 1024 * 1024
            || limits.short_text_bytes == 0
            || limits.short_text_bytes > limits.multiline_text_bytes
            || limits.multiline_text_bytes > limits.metadata_bytes
            || limits.attachment_bytes == 0
            || limits.attachment_bytes > limits.submission_bytes
            || limits.submission_bytes > HARD_MAX_BODY_BYTES
            || limits.png_pixels == 0
            || limits.png_pixels > HARD_MAX_PNG_PIXELS
            || limits.png_decoded_bytes == 0
            || limits.png_decoded_bytes > HARD_MAX_PNG_DECODED_BYTES;

        if invalid {
            return Err(AppError::InvalidRequest("Unsafe payload limits"));
        }

        Ok(())
    }

    fn validate_operational_limits(&self) -> Result<()> {
        let limits = &self.limits;
        let proof = &self.proof_of_work;
        let maintenance = &self.maintenance;

        let invalid = limits.concurrent_uploads_per_ip == 0
            || limits.concurrent_uploads_global < limits.concurrent_uploads_per_ip
            || limits.concurrent_uploads_global > 128
            || !(5..=300).contains(&limits.upload_timeout_seconds)
            || !(1..=1_000_000_000).contains(&proof.expected_work)
            || !(30..=3600).contains(&proof.challenge_lifetime_seconds)
            || !(30..=900).contains(&self.membership_recheck_seconds);

        let invalid = invalid
            || !(60..=86_400).contains(&maintenance.sweep_interval_seconds)
            || maintenance.staging_retention_seconds < limits.upload_timeout_seconds + 60
            || maintenance.staging_retention_seconds > 604_800
            || !(30..=36_500).contains(&maintenance.report_retention_days);

        if invalid {
            return Err(AppError::InvalidRequest("Unsafe operational limits"));
        }

        Ok(())
    }
}

fn validate_origin(value: &str, error_message: &'static str) -> Result<()> {
    let url = reqwest::Url::parse(value).map_err(|_| AppError::InvalidRequest(error_message))?;

    let allowed_scheme = url.scheme() == "https"
        || (url.scheme() == "http" && matches!(url.host_str(), Some("localhost" | "127.0.0.1")));

    let is_origin = url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none()
        && url.username().is_empty()
        && url.password().is_none();

    if !allowed_scheme || !is_origin {
        return Err(AppError::InvalidRequest(error_message));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::Value;

    use super::*;

    #[test]
    fn every_runtime_setting_has_contextual_help() {
        let configuration = serde_json::to_value(RuntimeConfiguration::default())
            .expect("serialize default runtime configuration");
        let mut configuration_paths = BTreeSet::new();
        collect_leaf_paths(&configuration, "", &mut configuration_paths);

        let documented_paths = SETTING_DEFINITIONS
            .iter()
            .map(|definition| definition.path.to_owned())
            .collect::<BTreeSet<_>>();

        assert_eq!(documented_paths, configuration_paths);
    }

    fn collect_leaf_paths(value: &Value, prefix: &str, paths: &mut BTreeSet<String>) {
        let Value::Object(object) = value else {
            return;
        };

        for (key, value) in object {
            let path = if prefix.is_empty() {
                key.to_owned()
            } else {
                format!("{prefix}.{key}")
            };

            if value.is_object() {
                collect_leaf_paths(value, &path, paths);
            } else {
                paths.insert(path);
            }
        }
    }
}
