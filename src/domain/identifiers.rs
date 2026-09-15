use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use uuid::{Uuid, Variant};

macro_rules! uuid_identifier {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, sqlx::Type)]
        #[sqlx(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }
        }

        impl TryFrom<Uuid> for $name {
            type Error = &'static str;

            fn try_from(value: Uuid) -> Result<Self, Self::Error> {
                if value.get_version_num() == 7 && value.get_variant() == Variant::RFC4122 {
                    Ok(Self(value))
                } else {
                    Err("identifier must be a UUIDv7")
                }
            }
        }

        impl FromStr for $name {
            type Err = &'static str;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                let uuid = Uuid::parse_str(value).map_err(|_| "identifier is not a UUID")?;
                Self::try_from(uuid)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let uuid = Uuid::deserialize(deserializer)?;
                Self::try_from(uuid).map_err(D::Error::custom)
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

uuid_identifier!(AttachmentId);
uuid_identifier!(ChallengeId);
uuid_identifier!(DiscordDeliveryLeaseId);
uuid_identifier!(GithubPublishAttemptId);
uuid_identifier!(IssueId);
uuid_identifier!(ReportId);
uuid_identifier!(SubmissionId);
uuid_identifier!(UploadId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_identifiers_are_uuid_v7() {
        assert_eq!(ReportId::new().0.get_version_num(), 7);
    }

    #[test]
    fn deserialization_rejects_other_uuid_versions() {
        let uuid_v4 = "550e8400-e29b-41d4-a716-446655440000";
        assert!(serde_json::from_str::<SubmissionId>(&format!("\"{uuid_v4}\"")).is_err());
    }
}
