use ladybird_reports::{
    domain::{AttachmentId, IssueId, ReportId, SubmissionId, UploadId},
    error::Result,
    infrastructure::{
        SecretCipher, database::AdminDatabase, hash_secret, random_token, read_secret,
    },
    runtime::required_environment,
};
use sha2::{Digest, Sha256};
use sqlx::postgres::PgPoolOptions;

const REPORT_ID: &str = "01a0a536-01cd-7ac7-a3cf-ae6a2d5030e5";
const SUBMISSION_ID: &str = "01a0a536-01d0-7c59-9b22-b9b210042528";
const STAGING_ID: &str = "01a0a536-01d2-7668-ad21-8f997d60ebb3";
const WEB_CONTENT_STACK: &str = include_str!("../../tests/fixtures/webcontent-stack.txt");

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::var("BROWSER_TEST_FIXTURE").as_deref() != Ok("enabled") {
        panic!("browser_test_fixture may only run with BROWSER_TEST_FIXTURE=enabled");
    }

    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&required_environment("ADMIN_DATABASE_URL")?)
        .await?;
    let session_token = random_token();
    let encrypted_access_token =
        SecretCipher::new(read_secret("SESSION_ENCRYPTION_KEY")?).encrypt("browser-test-token")?;

    let mut transaction = pool.begin().await?;

    // Browser tests own a deterministic view of the management database. The
    // PostgreSQL instance is disposable, but Rust integration tests may have
    // populated it earlier in the same local or CI test job.
    sqlx::query(
        "TRUNCATE
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
        "UPDATE runtime_configuration
         SET value = value || jsonb_build_object(
             'admin_base_url', 'http://127.0.0.1:3100',
             'github_reports_issue_field_id', 98500
         )
         WHERE singleton = true",
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
         VALUES ($1, 12345, $2, 'browser-test-csrf', now(), now() + interval '1 hour')
         ON CONFLICT (token_hash) DO NOTHING",
    )
    .bind(hash_secret(&session_token))
    .bind(encrypted_access_token)
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
            source_client_key,
            source_client_key_expires_at
         )
         VALUES (
            $1,
            $2,
            repeat('a', 64),
            'web_compat',
            '0.1.0-browser-test',
            'debug',
            'ready',
            $3,
            repeat('b', 64),
            now() + interval '30 days'
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
            ($1, 'platform', 'text', $3, true),
            ($1, 'architecture', 'text', $4, true),
            ($1, 'signal', 'text', $5, true),
            ($1, 'future-field', 'text', $6, false),
            ($1, 'git_commit', 'text', $7, true),
            ($1, 'build_configuration', 'text', $8, true),
            ($1, 'cpp_compiler', 'text', $9, true)
         ON CONFLICT (report_id, key) DO NOTHING",
    )
    .bind(REPORT_ID.parse::<ReportId>().expect("valid fixture UUIDv7"))
    .bind(serde_json::json!(WEB_CONTENT_STACK.trim()))
    .bind(serde_json::json!("macOS"))
    .bind(serde_json::json!("arm64"))
    .bind(serde_json::json!("SIGABRT"))
    .bind(serde_json::json!(
        "<script>window.fixtureWasExecuted = true</script>"
    ))
    .bind(serde_json::json!(
        "654cf9b187384fa8855eac4fbafaa70e75497083"
    ))
    .bind(serde_json::json!("debug"))
    .bind(serde_json::json!("AppleClang 21.0.0.21000101"))
    .execute(&mut *transaction)
    .await?;

    let attachment_id = AttachmentId::new();
    let attachment_bytes = vec![b'x'; 3756];
    let storage_key = format!("reports/{REPORT_ID}/{attachment_id}");
    let attachment_directory = std::path::PathBuf::from(required_environment("ATTACHMENT_ROOT")?)
        .join("reports")
        .join(REPORT_ID);
    if tokio::fs::try_exists(&attachment_directory).await? {
        tokio::fs::remove_dir_all(&attachment_directory).await?;
    }
    tokio::fs::create_dir_all(&attachment_directory).await?;
    tokio::fs::write(
        attachment_directory.join(attachment_id.to_string()),
        &attachment_bytes,
    )
    .await?;
    sqlx::query(
        "INSERT INTO attachments
            (id, report_id, client_id, name, media_type, size, sha256, storage_key)
         VALUES ($1, $2, $3, 'crash-diagnostics.txt', 'text/plain', $4, $5, $6)",
    )
    .bind(attachment_id)
    .bind(REPORT_ID.parse::<ReportId>().expect("valid fixture UUIDv7"))
    .bind(AttachmentId::new())
    .bind(attachment_bytes.len() as i64)
    .bind(hex::encode(Sha256::digest(&attachment_bytes)))
    .bind(storage_key)
    .execute(&mut *transaction)
    .await?;

    sqlx::query(
        "INSERT INTO audit_events (actor, action)
         VALUES (12345, 'session.signed_in')",
    )
    .execute(&mut *transaction)
    .await?;

    sqlx::query(
        "INSERT INTO audit_events (action, entity_id, details)
         VALUES ('report.submitted', $1, '{\"kind\": \"web_compat\"}'::jsonb)",
    )
    .bind(REPORT_ID.parse::<ReportId>().expect("valid fixture UUIDv7"))
    .execute(&mut *transaction)
    .await?;

    let existing_issue_id = IssueId::new();
    sqlx::query(
        "INSERT INTO issues (
            id,
            title,
            description,
            github_number,
            github_url,
            github_repository
         )
         VALUES (
            $1,
            'Intermittent navigation timeout',
            'Reports collected while investigating navigation stalls.',
            4812,
            'https://github.com/LadybirdBrowser/ladybird/issues/4812',
            'LadybirdBrowser/ladybird'
         )",
    )
    .bind(existing_issue_id)
    .execute(&mut *transaction)
    .await?;

    let example_reports = [
        ExampleReport {
            kind: "crash",
            client_version: "Ladybird Nightly 2026-09-15",
            build: "macOS · arm64 · Release",
            platform: "macOS",
            architecture: "arm64",
            hours_ago: 1,
            issue_id: None,
            confirmed: false,
        },
        ExampleReport {
            kind: "web_compat",
            client_version: "Ladybird Nightly 2026-09-15",
            build: "ConfirmedOS · arm64 · Release",
            platform: "ConfirmedOS",
            architecture: "arm64",
            hours_ago: 2,
            issue_id: None,
            confirmed: true,
        },
        ExampleReport {
            kind: "web_compat",
            client_version: "Ladybird Nightly 2026-09-15",
            build: "Linux · x86_64 · Debug",
            platform: "Linux",
            architecture: "x86_64",
            hours_ago: 3,
            issue_id: None,
            confirmed: false,
        },
        ExampleReport {
            kind: "crash",
            client_version: "0.7.0-alpha",
            build: "macOS · arm64 · ASan",
            platform: "macOS",
            architecture: "arm64",
            hours_ago: 7,
            issue_id: None,
            confirmed: false,
        },
        ExampleReport {
            kind: "web_compat",
            client_version: "Ladybird Nightly 2026-09-14",
            build: "Linux · x86_64 · Release",
            platform: "Linux",
            architecture: "x86_64",
            hours_ago: 18,
            issue_id: Some(existing_issue_id),
            confirmed: false,
        },
    ];

    for report in example_reports {
        insert_example_report(&mut transaction, report).await?;
    }

    // Keep one older, exact-signature report in triage for the issue view.
    // Historical reports are reviewed by a maintainer rather than auto-linked.
    sqlx::query(
        "UPDATE reports
         SET auto_match_eligible = false
         WHERE build = 'Linux · x86_64 · Debug'",
    )
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO report_fields
            (report_id, key, kind, value, recognized_at_submission)
         SELECT id, 'signal', 'text', '\"SIGABRT\"'::jsonb, true
         FROM reports
         WHERE build = 'Linux · x86_64 · Debug'",
    )
    .execute(&mut *transaction)
    .await?;

    for index in 0..52 {
        insert_example_report(
            &mut transaction,
            ExampleReport {
                kind: if index % 2 == 0 {
                    "crash"
                } else {
                    "web_compat"
                },
                client_version: "Pagination fixture",
                build: "PaginationOS · x86_64 · Release",
                platform: "PaginationOS",
                architecture: "x86_64",
                hours_ago: 24 + index,
                issue_id: None,
                confirmed: false,
            },
        )
        .await?;
    }

    transaction.commit().await?;

    // Exercise the same backfill used for reports submitted before deployment.
    let database = AdminDatabase::from_pool(pool);
    while database.index_pending_stack_traces().await? == 50 {}

    println!("{session_token}");
    Ok(())
}

#[derive(Clone, Copy)]
struct ExampleReport<'a> {
    kind: &'a str,
    client_version: &'a str,
    build: &'a str,
    platform: &'a str,
    architecture: &'a str,
    hours_ago: i32,
    issue_id: Option<IssueId>,
    confirmed: bool,
}

async fn insert_example_report(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    report: ExampleReport<'_>,
) -> Result<()> {
    let report_id = ReportId::new();
    let submission_id = SubmissionId::new();

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
            source_client_key,
            source_client_key_expires_at,
            issue_id,
            state,
            created_at,
            updated_at
         )
         VALUES (
            $1,
            $2,
            repeat('c', 64),
            $3,
            $4,
            $5,
            'ready',
            $6,
            repeat('d', 64),
            now() + interval '30 days',
            $7,
            CASE WHEN $7::uuid IS NOT NULL OR $9 THEN 'confirmed' ELSE 'triage' END,
            now() - make_interval(hours => $8),
            now() - make_interval(hours => $8)
         )",
    )
    .bind(report_id)
    .bind(submission_id)
    .bind(report.kind)
    .bind(report.client_version)
    .bind(report.build)
    .bind(UploadId::new())
    .bind(report.issue_id)
    .bind(report.hours_ago)
    .bind(report.confirmed)
    .execute(&mut **transaction)
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
            ($1, 'platform', 'text', $2, true),
            ($1, 'architecture', 'text', $3, true),
            ($1, 'stack', 'multiline', $4, true)",
    )
    .bind(report_id)
    .bind(serde_json::json!(report.platform))
    .bind(serde_json::json!(report.architecture))
    .bind(serde_json::json!(WEB_CONTENT_STACK.trim()))
    .execute(&mut **transaction)
    .await?;

    if report.issue_id.is_some() {
        sqlx::query(
            "INSERT INTO report_fields
                (report_id, key, kind, value, recognized_at_submission)
             VALUES ($1, 'signal', 'text', $2, true)",
        )
        .bind(report_id)
        .bind(serde_json::json!("SIGABRT"))
        .execute(&mut **transaction)
        .await?;
    }

    sqlx::query(
        "INSERT INTO audit_events (action, entity_id, details, created_at)
         VALUES (
            'report.submitted',
            $1,
            jsonb_build_object('kind', $2::text),
            now() - make_interval(hours => $3)
         )",
    )
    .bind(report_id)
    .bind(report.kind)
    .bind(report.hours_ago)
    .execute(&mut **transaction)
    .await?;

    Ok(())
}
