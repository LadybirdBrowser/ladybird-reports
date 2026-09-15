use std::{net::SocketAddr, sync::Arc};

use axum::{
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{Request, StatusCode, header::CONTENT_TYPE},
};
use ladybird_reports::{
    application::ReportIngestionService,
    domain::{
        DiagnosticField, FieldValue, ReportKind, ReportManifest, SubmissionId, proof_is_valid,
        sha256_hex,
    },
    infrastructure::{
        SecretCipher,
        attachments::FileAttachmentStore,
        database::{AdminDatabase, IngestDatabase, initialize_database},
    },
    web::public::{PublicState, router},
};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

#[tokio::test]
async fn generated_reporting_role_has_only_the_ingestion_surface() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("ladybird_reports=debug")
        .try_init();

    let Some(admin_url) = std::env::var("TEST_ADMIN_DATABASE_URL").ok() else {
        eprintln!("skipping PostgreSQL permission test: TEST_ADMIN_DATABASE_URL is unset");
        return;
    };

    let cipher = SecretCipher::new([19; 32]);
    let (admin_pool, bootstrap) = initialize_database(&admin_url, None, &cipher)
        .await
        .expect("initialize database");
    let reporting_url = bootstrap
        .generated_reporting_database_url
        .expect("reporting role was generated");
    let reporting_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&reporting_url)
        .await
        .expect("connect with generated reporting role");

    let configured: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM runtime_configuration WHERE singleton = true)",
    )
    .fetch_one(&reporting_pool)
    .await
    .expect("reporting role reads runtime configuration");
    assert!(configured);

    assert!(
        sqlx::query("SELECT * FROM reports LIMIT 1")
            .execute(&reporting_pool)
            .await
            .is_err(),
        "reporting role must not read reports directly"
    );
    assert!(
        sqlx::query(
            "INSERT INTO reports (
                id, submission_id, manifest_digest, kind, client_version,
                build, storage_state, staging_id
         )
             VALUES (
                '01a0a536-01d5-75ca-b634-044fcf62031d',
                '01a0a536-01d7-7cd4-b896-87f0f6ea6239',
                repeat('a', 64), 'web_compat', 'test', '', 'ready',
                '01a0a536-01da-727a-a015-ec57e365079a'
             )",
        )
        .execute(&reporting_pool)
        .await
        .is_err(),
        "reporting role must not insert reports directly"
    );

    assert!(
        sqlx::query(
            "INSERT INTO issues (id, title)
             VALUES ('550e8400-e29b-41d4-a716-446655440000', 'invalid identifier')",
        )
        .execute(&admin_pool)
        .await
        .is_err(),
        "database checks must reject UUID versions other than v7"
    );

    let (_, verified) = initialize_database(&admin_url, Some(&reporting_url), &cipher)
        .await
        .expect("verify configured reporting role");
    assert!(verified.generated_reporting_database_url.is_none());

    let pending_credentials: i64 = sqlx::query_scalar("SELECT count(*) FROM bootstrap_credentials")
        .fetch_one(&admin_pool)
        .await
        .expect("count bootstrap credentials");
    assert_eq!(pending_credentials, 0);

    sqlx::query(
        "UPDATE runtime_configuration
         SET value = jsonb_set(
            jsonb_set(value, '{proof_of_work,expected_work}', '1'),
            '{limits,minimum_free_storage_bytes}', '0'
         )",
    )
    .execute(&admin_pool)
    .await
    .expect("lower test-only proof and storage limits");

    let attachment_directory = tempfile::tempdir().expect("create attachment directory");
    let attachments = FileAttachmentStore::open(attachment_directory.path())
        .await
        .expect("open attachment store");
    let ingest_database = IngestDatabase::connect(&reporting_url)
        .await
        .expect("connect ingestion adapter");
    let ingestion = ReportIngestionService::new(ingest_database.clone(), attachments, [23; 32]);
    let manifest = ReportManifest {
        protocol: 1,
        submission_id: SubmissionId::new(),
        kind: ReportKind::WebCompat,
        client_version: "integration-test".into(),
        build: "Debug ARM64".into(),
        fields: vec![DiagnosticField {
            key: "future-client-field".into(),
            value: FieldValue::Text("accepted but marked unknown".into()),
        }],
        attachments: Vec::new(),
    };
    let manifest_bytes = serde_json::to_vec(&manifest).expect("serialize manifest");
    let manifest_digest = sha256_hex(&manifest_bytes);
    let application = router(PublicState {
        ingestion,
        database: ingest_database,
        client_address_key: Arc::new([29; 32]),
    });
    let peer = ConnectInfo("127.0.0.1:12345".parse::<SocketAddr>().unwrap());
    let challenge_request = Request::builder()
        .method("POST")
        .uri("/api/v1/challenges")
        .header(CONTENT_TYPE, "application/json")
        .extension(peer)
        .body(Body::from(
            serde_json::json!({ "manifest_digest": manifest_digest }).to_string(),
        ))
        .expect("build challenge request");
    let challenge_response = application
        .clone()
        .oneshot(challenge_request)
        .await
        .expect("challenge response");
    assert_eq!(challenge_response.status(), StatusCode::OK);
    let challenge: serde_json::Value = serde_json::from_slice(
        &to_bytes(challenge_response.into_body(), 64 * 1024)
            .await
            .expect("read challenge response"),
    )
    .expect("decode challenge response");
    let token = challenge["token"].as_str().expect("challenge token");
    let expected_work = challenge["expected_work"]
        .as_u64()
        .expect("challenge work factor");
    let nonce = (0..u64::MAX)
        .find(|nonce| proof_is_valid(token, *nonce, expected_work))
        .expect("find proof nonce");
    let boundary = "ladybird-reports-integration-test";
    let mut multipart = format!(
        "--{boundary}\r\n\
         Content-Disposition: form-data; name=\"manifest\"\r\n\
         Content-Type: application/json\r\n\r\n"
    )
    .into_bytes();
    multipart.extend_from_slice(&manifest_bytes);
    multipart.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let report_request = Request::builder()
        .method("POST")
        .uri("/api/v1/reports")
        .header(
            CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header("x-ladybird-challenge", token)
        .header("x-ladybird-nonce", nonce)
        .extension(peer)
        .body(Body::from(multipart))
        .expect("build report request");
    let report_response = application
        .clone()
        .oneshot(report_request)
        .await
        .expect("report response");
    assert_eq!(report_response.status(), StatusCode::OK);
    let receipt: serde_json::Value = serde_json::from_slice(
        &to_bytes(report_response.into_body(), 64 * 1024)
            .await
            .expect("read report response"),
    )
    .expect("decode report response");
    let report_id = receipt["receipt"]
        .as_str()
        .expect("report receipt")
        .parse::<ladybird_reports::domain::ReportId>()
        .expect("receipt is a report UUID");

    let admin_database = AdminDatabase::from_pool(admin_pool.clone());
    let report = admin_database
        .report_details(report_id)
        .await
        .expect("read report as administrator")
        .expect("accepted report exists");
    assert_eq!(report.report.client_version, "integration-test");
    assert_eq!(report.fields.len(), 1);
    assert!(!report.fields[0].recognized_at_submission);
    assert!(report.report.has_submission_source);
    assert!(!report.report.submission_source_is_blocked);

    sqlx::query("INSERT INTO maintainers (github_id, login) VALUES (999, 'integration-test')")
        .execute(&admin_pool)
        .await
        .expect("create maintainer for moderation action");

    admin_database
        .block_report_source(report_id, 999)
        .await
        .expect("block report source");

    let blocked_challenge_request = Request::builder()
        .method("POST")
        .uri("/api/v1/challenges")
        .header(CONTENT_TYPE, "application/json")
        .extension(peer)
        .body(Body::from(
            serde_json::json!({ "manifest_digest": manifest_digest }).to_string(),
        ))
        .expect("build blocked challenge request");
    let blocked_challenge_response = application
        .clone()
        .oneshot(blocked_challenge_request)
        .await
        .expect("blocked challenge response");
    assert_eq!(
        blocked_challenge_response.status(),
        StatusCode::TOO_MANY_REQUESTS
    );

    admin_database
        .unblock_report_source(report_id, 999)
        .await
        .expect("unblock report source");

    let unblocked_challenge_request = Request::builder()
        .method("POST")
        .uri("/api/v1/challenges")
        .header(CONTENT_TYPE, "application/json")
        .extension(peer)
        .body(Body::from(
            serde_json::json!({ "manifest_digest": manifest_digest }).to_string(),
        ))
        .expect("build unblocked challenge request");
    let unblocked_challenge_response = application
        .oneshot(unblocked_challenge_request)
        .await
        .expect("unblocked challenge response");
    assert_eq!(unblocked_challenge_response.status(), StatusCode::OK);
}
