use ipnet::IpNet;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

pub const HARD_MAX_BODY_BYTES: usize = 66 * 1024 * 1024;
pub const HARD_MAX_FIELDS: usize = 256;
pub const HARD_MAX_ATTACHMENTS: usize = 16;
pub const HARD_MAX_PNG_PIXELS: u64 = 50_000_000;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfiguration {
    pub public_base_url: String,
    pub admin_base_url: String,
    pub github_repository: String,
    pub trusted_proxies: Vec<IpNet>,
    pub limits: IngestionLimits,
    pub proof_of_work: ProofOfWorkConfiguration,
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
    pub upload_timeout_seconds: u64,
    pub minimum_free_storage_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProofOfWorkConfiguration {
    pub expected_work: u64,
    pub challenge_lifetime_seconds: u64,
}

impl Default for RuntimeConfiguration {
    fn default() -> Self {
        Self {
            public_base_url: "http://localhost:3001".into(),
            admin_base_url: "http://localhost:3000".into(),
            github_repository: "LadybirdBrowser/ladybird".into(),
            trusted_proxies: Vec::new(),
            limits: IngestionLimits::default(),
            proof_of_work: ProofOfWorkConfiguration {
                expected_work: 5_244_236,
                challenge_lifetime_seconds: 600,
            },
            membership_recheck_seconds: 300,
        }
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
            || limits.png_pixels > HARD_MAX_PNG_PIXELS;

        if invalid {
            return Err(AppError::InvalidRequest("Unsafe payload limits"));
        }

        Ok(())
    }

    fn validate_operational_limits(&self) -> Result<()> {
        let limits = &self.limits;
        let proof = &self.proof_of_work;

        let invalid = limits.concurrent_uploads_per_ip == 0
            || limits.concurrent_uploads_global < limits.concurrent_uploads_per_ip
            || limits.concurrent_uploads_global > 128
            || !(5..=300).contains(&limits.upload_timeout_seconds)
            || !(1..=1_000_000_000).contains(&proof.expected_work)
            || !(30..=3600).contains(&proof.challenge_lifetime_seconds)
            || !(30..=900).contains(&self.membership_recheck_seconds);

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
