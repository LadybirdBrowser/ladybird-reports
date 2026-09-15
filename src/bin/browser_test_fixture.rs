use ladybird_reports::{
    domain::{ReportId, SubmissionId, UploadId},
    error::Result,
    infrastructure::hash_secret,
    runtime::required_environment,
};
use sqlx::postgres::PgPoolOptions;

const REPORT_ID: &str = "01a0a536-01cd-7ac7-a3cf-ae6a2d5030e5";
const SUBMISSION_ID: &str = "01a0a536-01d0-7c59-9b22-b9b210042528";
const STAGING_ID: &str = "01a0a536-01d2-7668-ad21-8f997d60ebb3";
const SESSION_TOKEN: &str = "browser-test-session";

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::var("BROWSER_TEST_FIXTURE").as_deref() != Ok("enabled") {
        panic!("browser_test_fixture may only run with BROWSER_TEST_FIXTURE=enabled");
    }

    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&required_environment("ADMIN_DATABASE_URL")?)
        .await?;

    let mut transaction = pool.begin().await?;

    // Browser tests own a deterministic view of the management database. The
    // PostgreSQL instance is disposable, but Rust integration tests may have
    // populated it earlier in the same local or CI test job.
    sqlx::query(
        "TRUNCATE
            github_publish_attempts,
            audit_events,
            attachments,
            report_fields,
            reports,
            issues,
            sessions,
            maintainers
         RESTART IDENTITY CASCADE",
    )
    .execute(&mut *transaction)
    .await?;

    sqlx::query(
        "INSERT INTO maintainers (github_id, login)
         VALUES (12345, 'browser-tester')
         ON CONFLICT (github_id) DO UPDATE SET login = excluded.login",
    )
    .execute(&mut *transaction)
    .await?;

    sqlx::query(
        "INSERT INTO sessions (
            token_hash,
            github_id,
            encrypted_access_token,
            csrf_token,
            membership_verified_at,
            expires_at
         )
         VALUES ($1, 12345, 'unused-in-browser-tests', 'browser-test-csrf', now(), now() + interval '1 hour')
         ON CONFLICT (token_hash) DO NOTHING",
    )
    .bind(hash_secret(SESSION_TOKEN))
    .execute(&mut *transaction)
    .await?;

    sqlx::query(
        "INSERT INTO reports (
            id,
            submission_id,
            manifest_digest,
            kind,
            client_version,
            build,
            storage_state,
            staging_id,
            source_client_key
         )
         VALUES (
            $1,
            $2,
            repeat('a', 64),
            'web_compat',
            '0.1.0-browser-test',
            'Debug ARM64',
            'ready',
            $3,
            repeat('b', 64)
         )
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(REPORT_ID.parse::<ReportId>().expect("valid fixture UUIDv7"))
    .bind(
        SUBMISSION_ID
            .parse::<SubmissionId>()
            .expect("valid fixture UUIDv7"),
    )
    .bind(
        STAGING_ID
            .parse::<UploadId>()
            .expect("valid fixture UUIDv7"),
    )
    .execute(&mut *transaction)
    .await?;

    sqlx::query(
        "INSERT INTO report_fields (
            report_id,
            key,
            kind,
            value,
            recognized_at_submission
         )
         VALUES
            ($1, 'stack', 'multiline', $2, true),
            ($1, 'future-field', 'text', $3, false)
         ON CONFLICT (report_id, key) DO NOTHING",
    )
    .bind(REPORT_ID.parse::<ReportId>().expect("valid fixture UUIDv7"))
    .bind(serde_json::json!("ladybird!WebContentMain + 42"))
    .bind(serde_json::json!(
        "<script>window.fixtureWasExecuted = true</script>"
    ))
    .execute(&mut *transaction)
    .await?;

    transaction.commit().await?;
    Ok(())
}
