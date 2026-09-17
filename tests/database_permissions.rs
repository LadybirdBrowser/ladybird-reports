use std::{net::SocketAddr, sync::Arc};

use axum::{
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{Request, StatusCode, header::CONTENT_TYPE},
};
use chrono::{Duration, Utc};
use ladybird_reports::{
    application::ReportIngestionService,
    domain::{
        DiagnosticField, FieldValue, IssueId, ReportId, ReportKind, ReportManifest, SubmissionId,
        UploadId, proof_is_valid, sha256_hex,
    },
    infrastructure::{
        SecretCipher,
        attachments::FileAttachmentStore,
        database::{AdminDatabase, IngestDatabase, NewSession, initialize_database},
        github::{GithubIssue, GithubIssueState},
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
    let obsolete_report_columns: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM information_schema.columns
         WHERE (table_name = 'reports'
                AND column_name IN ('deleted_at', 'hidden_at', 'confirmed_at', 'assigned_at'))
            OR (table_name = 'attachments' AND column_name = 'deleted_at')",
    )
    .fetch_one(&admin_pool)
    .await
    .expect("inspect report schema");
    assert_eq!(obsolete_report_columns, 0);
    let reporting_url = bootstrap
        .generated_reporting_database_url
        .expect("reporting role was generated");
    let reporting_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&reporting_url)
        .await
        .expect("connect with generated reporting role");

    let reporting_configuration: serde_json::Value =
        sqlx::query_scalar("SELECT reporting_runtime_configuration()")
            .fetch_one(&reporting_pool)
            .await
            .expect("reporting role reads its runtime configuration projection");
    assert!(reporting_configuration.get("limits").is_some());
    assert!(reporting_configuration.get("discord").is_none());

    assert!(
        sqlx::query("SELECT value FROM runtime_configuration")
            .execute(&reporting_pool)
            .await
            .is_err(),
        "reporting role must not read configuration secrets directly"
    );

    let configured: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM field_definitions)")
        .fetch_one(&reporting_pool)
        .await
        .expect("reporting role reads field definitions");
    assert!(configured);

    let signal_number_kind: String =
        sqlx::query_scalar("SELECT kind FROM field_definitions WHERE key = 'signal_number'")
            .fetch_one(&reporting_pool)
            .await
            .expect("reporting role reads the numeric signal definition");
    assert_eq!(signal_number_kind, "number");

    assert!(
        sqlx::query("SELECT * FROM report_stack_signatures LIMIT 1")
            .execute(&reporting_pool)
            .await
            .is_err(),
        "reporting role cannot read derived stack signatures"
    );

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
            "INSERT INTO issues (
                id, title, github_number, github_url,
                github_repository
             )
             VALUES (
                '550e8400-e29b-41d4-a716-446655440000',
                'invalid identifier',
                9000,
                'https://github.com/LadybirdBrowser/ladybird/issues/9000',
                'LadybirdBrowser/ladybird'
             )",
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
    let ingestion =
        ReportIngestionService::new(ingest_database.clone(), attachments.clone(), [23; 32]);
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
        ingestion: ingestion.clone(),
        database: ingest_database.clone(),
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
    let oauth_state_hash = "1".repeat(64);
    let requested_report = "/reports/01a0a536-01cd-7ac7-a3cf-ae6a2d5030e5?source=discord";
    admin_database
        .store_oauth_state(&oauth_state_hash, requested_report)
        .await
        .expect("store requested URL with sign-in state");
    assert_eq!(
        admin_database
            .consume_oauth_state(&oauth_state_hash)
            .await
            .expect("consume sign-in state"),
        Some(requested_report.to_owned())
    );
    assert_eq!(
        admin_database
            .consume_oauth_state(&oauth_state_hash)
            .await
            .expect("check one-time sign-in state"),
        None
    );
    let report = admin_database
        .report_details(report_id)
        .await
        .expect("read report as administrator")
        .expect("accepted report exists");
    assert_eq!(report.report.client_version, "integration-test");
    assert_eq!(report.fields.len(), 1);
    assert!(!report.fields[0].recognized_at_submission);
    assert!(report.report.has_submission_source);
    assert_eq!(report.report.source_ip.as_deref(), Some("127.0.0.1"));
    assert!(!report.report.submission_source_is_blocked);
    assert!(report.report.expires_at.is_some());
    assert!(
        report
            .events
            .iter()
            .any(|event| event.action == "report.submitted")
    );

    sqlx::query("INSERT INTO maintainers (github_id, login) VALUES (999, 'integration-test')")
        .execute(&admin_pool)
        .await
        .expect("create maintainer for moderation action");

    let original_team = admin_database
        .configuration()
        .await
        .expect("load original access team")
        .github_authorization_team;

    let integration_session_hash = "9".repeat(64);
    admin_database
        .create_session(NewSession {
            github_id: 999,
            login: "integration-test",
            authorized_team: &original_team,
            token_hash: &integration_session_hash,
            encrypted_access_token: "encrypted-token",
            csrf_token: "integration-csrf",
            lifetime: Duration::minutes(5),
        })
        .await
        .expect("create audited session");
    admin_database
        .delete_session(&integration_session_hash, 999)
        .await
        .expect("delete audited session");

    let session_audit_count: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM audit_events
         WHERE actor = 999
            AND action IN ('session.signed_in', 'session.signed_out')",
    )
    .fetch_one(&admin_pool)
    .await
    .expect("count session audit events");
    assert_eq!(session_audit_count, 2);

    let team_change_session_hash = "8".repeat(64);
    admin_database
        .create_session(NewSession {
            github_id: 999,
            login: "integration-test",
            authorized_team: &original_team,
            token_hash: &team_change_session_hash,
            encrypted_access_token: "encrypted-token",
            csrf_token: "integration-csrf",
            lifetime: Duration::minutes(5),
        })
        .await
        .expect("create session under original team policy");

    let mut configuration = admin_database
        .configuration()
        .await
        .expect("load runtime configuration");
    let admin_listener = admin_database
        .start_configuration_cache()
        .await
        .expect("start admin configuration listener");
    let reporting_listener = ingest_database
        .start_configuration_cache()
        .await
        .expect("start reporting configuration listener");
    configuration.github_authorization_team = "ExampleOrg/reports-reviewers".into();
    admin_database
        .update_configuration(&configuration, 999)
        .await
        .expect("change access team");
    assert!(
        admin_database
            .find_session(&team_change_session_hash)
            .await
            .expect("look up former session")
            .is_none(),
        "changing the access team must revoke previously verified sessions"
    );
    assert_eq!(
        admin_database
            .configuration()
            .await
            .expect("load updated configuration")
            .github_authorization_team,
        "ExampleOrg/reports-reviewers"
    );
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if ingest_database
                .configuration()
                .await
                .expect("read cached reporting configuration")
                .github_authorization_team
                == "ExampleOrg/reports-reviewers"
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("reporting process received configuration notification");
    assert!(
        admin_database
            .create_session(NewSession {
                github_id: 999,
                login: "integration-test",
                authorized_team: &original_team,
                token_hash: &"7".repeat(64),
                encrypted_access_token: "encrypted-token",
                csrf_token: "integration-csrf",
                lifetime: Duration::minutes(5),
            })
            .await
            .is_err(),
        "a login verified under the old team must be rejected"
    );
    configuration.github_authorization_team = original_team;
    admin_database
        .update_configuration(&configuration, 999)
        .await
        .expect("restore access team for browser tests");
    admin_listener.abort();
    reporting_listener.abort();

    attachments
        .remove_report(report_id)
        .await
        .expect("remove expired report files");
    admin_database
        .block_report_source(report_id, 999, false)
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

    sqlx::raw_sql(
        "UPDATE challenges SET expires_at = now() - interval '1 second';
         UPDATE rate_buckets SET expires_at = now() - interval '1 second';
         INSERT INTO oauth_states (state_hash, expires_at)
         VALUES (repeat('e', 64), now() - interval '1 second');
         INSERT INTO sessions (
            token_hash, github_id, encrypted_access_token, csrf_token, expires_at
         ) VALUES (
            repeat('f', 64), 999, 'expired-token', 'expired-csrf',
            now() - interval '1 second'
         )",
    )
    .execute(&admin_pool)
    .await
    .expect("create expired maintenance records");

    let ingestion_sweep = ingest_database
        .sweep_expired_state()
        .await
        .expect("sweep expired ingestion state");
    assert!(ingestion_sweep.challenges_deleted >= 2);
    assert!(ingestion_sweep.rate_buckets_deleted >= 1);

    sqlx::query("UPDATE reports SET expires_at = now() - interval '1 second' WHERE id = $1")
        .bind(report_id)
        .execute(&admin_pool)
        .await
        .expect("expire accepted report");
    let admin_sweep = admin_database
        .begin_maintenance_sweep(3_650)
        .await
        .expect("begin admin maintenance sweep");
    assert_eq!(admin_sweep.sessions_deleted, 1);
    assert_eq!(admin_sweep.oauth_states_deleted, 1);
    assert!(admin_sweep.reports_ready_for_purge.contains(&report_id));

    admin_database
        .finish_report_purge(report_id)
        .await
        .expect("purge expired report");
    assert!(
        admin_database
            .report_details(report_id)
            .await
            .expect("query purged report")
            .is_none()
    );

    let issue_id = IssueId::new();
    sqlx::query(
        "INSERT INTO issues (
            id, title, github_number, github_url,
            github_repository
         )
         VALUES (
            $1,
            'Assigned integration report',
            4812,
            'https://github.com/LadybirdBrowser/ladybird/issues/4812',
            'LadybirdBrowser/ladybird'
         )",
    )
    .bind(issue_id)
    .execute(&admin_pool)
    .await
    .expect("create issue for source block test");

    let first_triage_report = ReportId::new();
    let second_triage_report = ReportId::new();
    let other_triage_report = ReportId::new();
    let assigned_report = ReportId::new();
    for (test_report_id, assigned_issue) in [
        (first_triage_report, None),
        (second_triage_report, None),
        (other_triage_report, None),
        (assigned_report, Some(issue_id)),
    ] {
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
                source_ip,
                issue_id,
                state
             ) VALUES (
                $1,
                $2,
                repeat('7', 64),
                'crash',
                'source-block-test',
                '',
                'ready',
                $3,
                repeat('8', 64),
                '203.0.113.10'::inet,
                $4,
                CASE WHEN $4::uuid IS NULL THEN 'triage' ELSE 'confirmed' END
             )",
        )
        .bind(test_report_id)
        .bind(SubmissionId::new())
        .bind(UploadId::new())
        .bind(assigned_issue)
        .execute(&admin_pool)
        .await
        .expect("create report for source block test");
    }

    admin_database
        .set_report_state(second_triage_report, "confirmed", 999)
        .await
        .expect("confirm one report from the blocked source");

    let block_outcome = admin_database
        .block_report_source(first_triage_report, 999, true)
        .await
        .expect("block source and reject its triage reports");
    assert_eq!(block_outcome.rejected_triage_reports, 1);

    let rejected_triage_reports: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM reports
         WHERE source_client_key = repeat('8', 64)
            AND issue_id IS NULL
            AND state = 'rejected'",
    )
    .fetch_one(&admin_pool)
    .await
    .expect("count rejected triage reports");
    assert_eq!(rejected_triage_reports, 2);

    let confirmed_report_is_visible: bool = sqlx::query_scalar(
        "SELECT state = 'confirmed'
         FROM reports
         WHERE id = $1",
    )
    .bind(second_triage_report)
    .fetch_one(&admin_pool)
    .await
    .expect("check confirmed report after source block");
    assert!(confirmed_report_is_visible);

    admin_database
        .set_report_state(second_triage_report, "rejected", 999)
        .await
        .expect("reject the confirmed report");
    let rejected_report_is_retained: bool = sqlx::query_scalar(
        "SELECT state = 'rejected'
         FROM reports
         WHERE id = $1",
    )
    .bind(second_triage_report)
    .fetch_one(&admin_pool)
    .await
    .expect("check rejected report retention state");
    assert!(rejected_report_is_retained);

    let assigned_report_is_visible: bool = sqlx::query_scalar(
        "SELECT state = 'confirmed'
         FROM reports
         WHERE id = $1",
    )
    .bind(assigned_report)
    .fetch_one(&admin_pool)
    .await
    .expect("check assigned report after source block");
    assert!(assigned_report_is_visible);
    assert!(
        admin_database
            .set_report_state(assigned_report, "triage", 999)
            .await
            .is_err(),
        "a linked report cannot return to triage"
    );

    let first_github_report = ReportId::new();
    let second_github_report = ReportId::new();
    let third_github_report = ReportId::new();
    for github_report in [
        first_github_report,
        second_github_report,
        third_github_report,
    ] {
        sqlx::query(
            "INSERT INTO reports (
                id,
                submission_id,
                manifest_digest,
                kind,
                client_version,
                build,
                storage_state,
                staging_id
             ) VALUES (
                $1,
                $2,
                repeat('9', 64),
                'crash',
                'github-link-test',
                '',
                'ready',
                $3
             )",
        )
        .bind(github_report)
        .bind(SubmissionId::new())
        .bind(UploadId::new())
        .execute(&admin_pool)
        .await
        .expect("create report for GitHub issue assignment test");
    }

    let reused_issue = admin_database
        .assign_report_to_github_issue(
            &github_issue(4812, "Assigned integration report"),
            first_github_report,
            "LadybirdBrowser/ladybird",
            999,
        )
        .await
        .expect("assign report to issue already linked to GitHub");
    assert_eq!(reused_issue.issue_id, issue_id);
    assert!(!reused_issue.created);

    let created_issue = admin_database
        .assign_report_to_github_issue(
            &github_issue(4813, "New linked issue"),
            second_github_report,
            "LadybirdBrowser/ladybird",
            999,
        )
        .await
        .expect("create issue for an unlinked GitHub issue");
    assert!(created_issue.created);

    let mut edited_github_issue = github_issue(4813, "Renamed on GitHub");
    edited_github_issue.body = Some("Description edited on GitHub.".into());
    let reused_created_issue = admin_database
        .assign_report_to_github_issue(
            &edited_github_issue,
            third_github_report,
            "LadybirdBrowser/ladybird",
            999,
        )
        .await
        .expect("reuse issue created for the same GitHub issue");
    assert_eq!(reused_created_issue.issue_id, created_issue.issue_id);
    assert!(!reused_created_issue.created);
    let assigned_states: Vec<String> =
        sqlx::query_scalar("SELECT state FROM reports WHERE id = ANY($1) ORDER BY id")
            .bind(
                &[
                    first_github_report,
                    second_github_report,
                    third_github_report,
                ][..],
            )
            .fetch_all(&admin_pool)
            .await
            .expect("read states after GitHub issue assignments");
    assert_eq!(assigned_states, vec!["confirmed"; 3]);
    let synchronized_issue = admin_database
        .find_issue(created_issue.issue_id)
        .await
        .expect("load synchronized issue")
        .expect("tracked issue exists");
    assert_eq!(synchronized_issue.title, "Renamed on GitHub");
    assert_eq!(
        synchronized_issue.description,
        "Description edited on GitHub."
    );
    let sync_audit: serde_json::Value = sqlx::query_scalar(
        "SELECT details FROM audit_events
         WHERE entity_id = $1 AND action = 'issue.sync_github'
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(created_issue.issue_id)
    .fetch_one(&admin_pool)
    .await
    .expect("read GitHub issue sync audit");
    assert_eq!(sync_audit["title_changed"], true);
    assert_eq!(sync_audit["description_changed"], true);

    let linked_issues = admin_database
        .issues_linked_to_github_numbers("LadybirdBrowser/ladybird", &[4812, 4813, 9999])
        .await
        .expect("load internal issues for GitHub search results");
    assert_eq!(linked_issues.len(), 2);
    assert!(linked_issues.iter().any(|link| {
        link.issue_id == issue_id
            && link.github_number == 4812
            && link.title == "Assigned integration report"
    }));

    let active_issue_count: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM issues
         WHERE github_number = 4813
            AND merged_into IS NULL",
    )
    .fetch_one(&admin_pool)
    .await
    .expect("count active internal issues for one GitHub issue");
    assert_eq!(active_issue_count, 1);

    let created_issue_audit: serde_json::Value = sqlx::query_scalar(
        "SELECT details
         FROM audit_events
         WHERE entity_id = $1 AND action = 'issue.create'",
    )
    .bind(created_issue.issue_id)
    .fetch_one(&admin_pool)
    .await
    .expect("read issue creation audit");
    assert_eq!(created_issue_audit["github_number"], 4813);
    assert_eq!(
        created_issue_audit["report_id"],
        second_github_report.to_string()
    );

    let mut closed_github_issue = github_issue(4813, "New linked issue");
    closed_github_issue.state = GithubIssueState::Closed;
    admin_database
        .sync_github_issue(
            "LadybirdBrowser/ladybird",
            &closed_github_issue,
            None,
            "test",
        )
        .await
        .expect("synchronize GitHub closure");
    let resolved_at: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT resolved_at FROM issues WHERE id = $1")
            .bind(created_issue.issue_id)
            .fetch_one(&admin_pool)
            .await
            .expect("check synchronized issue state");
    assert!(resolved_at.is_some());
    assert!(
        admin_database
            .assign_report_to_issue(first_github_report, created_issue.issue_id, 999)
            .await
            .is_err(),
        "reports cannot be added to a closed GitHub issue"
    );

    let mut transferred_issue = github_issue(8888, "Transferred GitHub issue");
    transferred_issue.id = 94813;
    transferred_issue.html_url = "https://github.com/LadybirdBrowser/other/issues/8888".into();
    admin_database
        .sync_github_issue("LadybirdBrowser/other", &transferred_issue, None, "test")
        .await
        .expect("detect issue transfer by stable GitHub identity");
    assert!(
        admin_database
            .assign_report_to_github_issue(
                &transferred_issue,
                first_github_report,
                "LadybirdBrowser/other",
                999,
            )
            .await
            .is_err(),
        "transferred GitHub issue must not create a second Reports issue"
    );

    admin_database
        .mark_github_issue_unavailable(
            "LadybirdBrowser/ladybird",
            4813,
            Some(94813),
            "missing",
            "test",
        )
        .await
        .expect("mark deleted GitHub issue");
    admin_database
        .sync_github_issue(
            "LadybirdBrowser/ladybird",
            &github_issue(4813, "Stale webhook title"),
            None,
            "webhook",
        )
        .await
        .expect("ignore a late update after deletion");
    let github_state: String = sqlx::query_scalar("SELECT github_state FROM issues WHERE id = $1")
        .bind(created_issue.issue_id)
        .fetch_one(&admin_pool)
        .await
        .expect("check GitHub state after stale webhook");
    assert_eq!(github_state, "missing");
    admin_database
        .replace_github_issue(
            created_issue.issue_id,
            "LadybirdBrowser/ladybird",
            &github_issue(4814, "Replacement GitHub issue"),
            999,
        )
        .await
        .expect("replace deleted GitHub link");
    let old_github_link = admin_database
        .issues_linked_to_github_numbers("LadybirdBrowser/ladybird", &[4813])
        .await
        .expect("resolve historical GitHub link");
    assert_eq!(old_github_link.len(), 1);
    assert_eq!(old_github_link[0].issue_id, created_issue.issue_id);

    admin_database
        .merge_issue(created_issue.issue_id, issue_id, 999)
        .await
        .expect("merge issues with per-report audits");
    let merged_report_audits: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM audit_events
         WHERE action = 'report.update_issue'
            AND entity_id = ANY($1::uuid[])
            AND details->>'from' = $2
            AND details->>'to' = $3",
    )
    .bind(vec![second_github_report.0, third_github_report.0])
    .bind(created_issue.issue_id.to_string())
    .bind(issue_id.to_string())
    .fetch_one(&admin_pool)
    .await
    .expect("count report reassignment audits from merge");
    assert_eq!(merged_report_audits, 2);

    let merge_audit: serde_json::Value = sqlx::query_scalar(
        "SELECT details
         FROM audit_events
         WHERE entity_id = $1 AND action = 'issue.merge'",
    )
    .bind(created_issue.issue_id)
    .fetch_one(&admin_pool)
    .await
    .expect("read issue merge audit");
    assert_eq!(merge_audit["into"], issue_id.to_string());
    assert_eq!(merge_audit["reports_moved"], 2);

    let merged_github_link = admin_database
        .issues_linked_to_github_numbers("LadybirdBrowser/ladybird", &[4813, 4814])
        .await
        .expect("resolve both historical links after merge");
    assert_eq!(merged_github_link.len(), 2);
    assert!(
        merged_github_link
            .iter()
            .all(|link| link.issue_id == issue_id)
    );

    let linked_before_delete: i64 =
        sqlx::query_scalar("SELECT count(*) FROM reports WHERE issue_id = $1")
            .bind(issue_id)
            .fetch_one(&admin_pool)
            .await
            .expect("count reports linked to issue before deletion");
    assert!(linked_before_delete > 1);

    admin_database
        .unlink_report_from_issue(issue_id, first_github_report, 999)
        .await
        .expect("unlink one report from its issue");
    let unlinked_assignment: Option<IssueId> =
        sqlx::query_scalar("SELECT issue_id FROM reports WHERE id = $1")
            .bind(first_github_report)
            .fetch_one(&admin_pool)
            .await
            .expect("check unlinked report");
    assert_eq!(unlinked_assignment, None);

    let reports_unlinked = admin_database
        .hide_issue(issue_id, 999)
        .await
        .expect("hide tracked issue and unlink its reports");
    assert_eq!(reports_unlinked as i64, linked_before_delete - 1);
    assert!(
        admin_database
            .find_issue(issue_id)
            .await
            .expect("look up hidden issue")
            .is_none()
    );
    assert!(
        admin_database
            .find_issue(created_issue.issue_id)
            .await
            .expect("look up merged issue hidden with destination")
            .is_none()
    );
    let remaining_assignments: i64 =
        sqlx::query_scalar("SELECT count(*) FROM reports WHERE issue_id = $1")
            .bind(issue_id)
            .fetch_one(&admin_pool)
            .await
            .expect("count assignments to hidden issue");
    assert_eq!(remaining_assignments, 0);
    let hide_audit: serde_json::Value = sqlx::query_scalar(
        "SELECT details FROM audit_events
         WHERE entity_id = $1 AND action = 'issue.update_visibility'",
    )
    .bind(issue_id)
    .fetch_one(&admin_pool)
    .await
    .expect("read issue deletion audit");
    assert_eq!(hide_audit["reports_unlinked"], reports_unlinked);

    let retracked = admin_database
        .assign_report_to_github_issue(
            &github_issue(4812, "Re-tracked GitHub issue"),
            first_github_report,
            "LadybirdBrowser/ladybird",
            999,
        )
        .await
        .expect("track the same GitHub issue after hiding its old record");
    assert!(retracked.created);
    assert_ne!(retracked.issue_id, issue_id);

    sqlx::query("DELETE FROM discord_report_notifications")
        .execute(&admin_pool)
        .await
        .expect("clear notification queue before focused checks");
    sqlx::query(
        "UPDATE discord_delivery_state
         SET
            lease_id = NULL,
            leased_report_id = NULL,
            lease_expires_at = NULL,
            paused_until = NULL,
            consecutive_failures = 0",
    )
    .execute(&admin_pool)
    .await
    .expect("reset Discord delivery state before focused checks");

    let rolled_back_report = ReportId::new();
    let mut rolled_back_transaction = admin_pool
        .begin()
        .await
        .expect("begin rolled-back report transaction");
    sqlx::query(
        "INSERT INTO reports (
            id, submission_id, manifest_digest, kind, client_version,
            build, storage_state, staging_id
         ) VALUES ($1, $2, repeat('a', 64), 'crash', 'rolled-back', '', 'ready', $3)",
    )
    .bind(rolled_back_report)
    .bind(SubmissionId::new())
    .bind(UploadId::new())
    .execute(&mut *rolled_back_transaction)
    .await
    .expect("insert report inside transaction");
    rolled_back_transaction
        .rollback()
        .await
        .expect("roll back report transaction");

    let rolled_back_notification_count: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM discord_report_notifications
         WHERE report_id = $1",
    )
    .bind(rolled_back_report)
    .fetch_one(&admin_pool)
    .await
    .expect("count notifications for rolled-back report");
    assert_eq!(rolled_back_notification_count, 0);

    let first_notification_report = ReportId::new();
    let second_notification_report = ReportId::new();
    for notification_report in [first_notification_report, second_notification_report] {
        sqlx::query(
            "INSERT INTO reports (
                id, submission_id, manifest_digest, kind, client_version,
                build, storage_state, staging_id
             ) VALUES (
                $1, $2, repeat('b', 64), 'crash',
                'notification-test', 'macOS arm64', 'ready', $3
             )",
        )
        .bind(notification_report)
        .bind(SubmissionId::new())
        .bind(UploadId::new())
        .execute(&admin_pool)
        .await
        .expect("create report with transactional Discord notification");
    }

    let first_notification = admin_database
        .claim_discord_notification(40)
        .await
        .expect("claim first Discord notification")
        .expect("first Discord notification exists");
    assert_eq!(first_notification.attempt_count, 0);
    assert_eq!(first_notification.client_version, "notification-test");

    assert!(
        admin_database
            .defer_discord_notification(
                first_notification.report_id,
                first_notification.lease_id,
                30,
                Some(503),
                "response",
            )
            .await
            .expect("defer first Discord notification")
    );
    assert!(
        admin_database
            .claim_discord_notification(40)
            .await
            .expect("check globally paused Discord queue")
            .is_none()
    );

    sqlx::query("UPDATE discord_delivery_state SET paused_until = now()")
        .execute(&admin_pool)
        .await
        .expect("resume Discord queue for integration test");
    let retried_notification = admin_database
        .claim_discord_notification(40)
        .await
        .expect("reclaim deferred Discord notification")
        .expect("deferred Discord notification remains queued");
    assert_eq!(retried_notification.report_id, first_notification.report_id);
    assert_eq!(retried_notification.attempt_count, 1);
    assert!(
        admin_database
            .finish_discord_notification(
                retried_notification.report_id,
                retried_notification.lease_id,
                0,
            )
            .await
            .expect("finish retried Discord notification")
    );

    let second_notification = admin_database
        .claim_discord_notification(40)
        .await
        .expect("claim second Discord notification")
        .expect("second Discord notification exists");
    assert_ne!(second_notification.report_id, first_notification.report_id);
    assert!(
        admin_database
            .finish_discord_notification(
                second_notification.report_id,
                second_notification.lease_id,
                0,
            )
            .await
            .expect("finish second Discord notification")
    );

    assert!(
        admin_database
            .claim_discord_notification(40)
            .await
            .expect("check drained Discord queue")
            .is_none()
    );

    // The backfill interprets old multiline stacks and new stack_trace fields
    // without changing either submitted value or assigning either report.
    let original_stacks = [
        "#0 abcdef1234567890 0x100 Web::Window::close() at /old/WebContent\n#1 abcdef1234567890 0x200 Web::Page::destroy() at /old/WebContent",
        "#0 1234567890abcdef 0x999 Web::Window::close() at /new/WebContent\n#1 1234567890abcdef 0xaaa Web::Page::destroy() at /new/WebContent",
    ];
    let stack_reports = [ReportId::new(), ReportId::new()];
    for (index, stack_report) in stack_reports.iter().enumerate() {
        sqlx::query(
            "INSERT INTO reports (
                id, submission_id, manifest_digest, kind, client_version,
                build, storage_state, staging_id
             ) VALUES (
                $1, $2, repeat('c', 64), 'crash',
                'stack-index-test', 'macOS arm64', 'ready', $3
             )",
        )
        .bind(stack_report)
        .bind(SubmissionId::new())
        .bind(UploadId::new())
        .execute(&admin_pool)
        .await
        .expect("create stack report");
        sqlx::query(
            "INSERT INTO report_fields
                (report_id, key, kind, value, recognized_at_submission)
             VALUES ($1, 'stack', $2, $3, true)",
        )
        .bind(stack_report)
        .bind(if index == 0 {
            "multiline"
        } else {
            "stack_trace"
        })
        .bind(serde_json::json!(original_stacks[index]))
        .execute(&admin_pool)
        .await
        .expect("store original stack text");
    }

    assert_eq!(
        admin_database
            .index_pending_stack_traces()
            .await
            .expect("backfill existing reports"),
        2
    );
    let candidates = admin_database
        .similar_reports(stack_reports[0])
        .await
        .expect("find matching stack signatures");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].report_id, stack_reports[1]);
    assert!(candidates[0].exact);
    assert_eq!(candidates[0].matching_frames, 2);
    let stored_stack: serde_json::Value = sqlx::query_scalar(
        "SELECT value FROM report_fields WHERE report_id = $1 AND key = 'stack'",
    )
    .bind(stack_reports[0])
    .fetch_one(&admin_pool)
    .await
    .expect("read unchanged submitted stack");
    assert_eq!(stored_stack, serde_json::json!(original_stacks[0]));
    let still_unassigned: i64 =
        sqlx::query_scalar("SELECT count(*) FROM reports WHERE id = ANY($1) AND issue_id IS NULL")
            .bind(&stack_reports[..])
            .fetch_one(&admin_pool)
            .await
            .expect("check matching never assigns reports");
    assert_eq!(still_unassigned, 2);

    sqlx::query("UPDATE report_stack_signatures SET algorithm_version = 0 WHERE report_id = $1")
        .bind(stack_reports[0])
        .execute(&admin_pool)
        .await
        .expect("mark an old signature for regeneration");
    assert_eq!(
        admin_database
            .index_pending_stack_traces()
            .await
            .expect("reprocess outdated signature"),
        1
    );
    let current_version: i32 = sqlx::query_scalar(
        "SELECT algorithm_version FROM report_stack_signatures WHERE report_id = $1",
    )
    .bind(stack_reports[0])
    .fetch_one(&admin_pool)
    .await
    .expect("read regenerated signature");
    assert_eq!(current_version, 1);

    let matching_issue = IssueId::new();
    sqlx::query(
        "INSERT INTO issues (
            id, title, github_number, github_url,
            github_repository, github_state
         ) VALUES (
            $1, 'Matching stack', 60001,
            'https://github.com/LadybirdBrowser/ladybird/issues/60001',
            'LadybirdBrowser/ladybird', 'open'
         )",
    )
    .bind(matching_issue)
    .execute(&admin_pool)
    .await
    .expect("create open issue for signature matching");
    admin_database
        .assign_report_to_issue(stack_reports[0], matching_issue, 999)
        .await
        .expect("link first report manually");

    admin_database
        .set_report_state(stack_reports[0], "rejected", 999)
        .await
        .expect("reject a linked report");
    let unmatched_report =
        insert_stack_report_for_matching(&admin_pool, original_stacks[1], 0).await;
    admin_database
        .index_report_stack_traces(unmatched_report)
        .await
        .expect("index report while its matching issue report is rejected");
    let unmatched_issue: Option<IssueId> =
        sqlx::query_scalar("SELECT issue_id FROM reports WHERE id = $1")
            .bind(unmatched_report)
            .fetch_one(&admin_pool)
            .await
            .expect("read issue for unmatched report");
    assert_eq!(unmatched_issue, None);

    admin_database
        .assign_report_to_issue(stack_reports[0], matching_issue, 999)
        .await
        .expect("reconfirm a report by adding it to the same issue");
    let restored_state: String = sqlx::query_scalar("SELECT state FROM reports WHERE id = $1")
        .bind(stack_reports[0])
        .fetch_one(&admin_pool)
        .await
        .expect("read restored report state");
    assert_eq!(restored_state, "confirmed");

    let matching_report =
        insert_stack_report_for_matching(&admin_pool, original_stacks[1], 0).await;
    let queued_before_indexing: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM discord_report_notifications WHERE report_id = $1
         )",
    )
    .bind(matching_report)
    .fetch_one(&admin_pool)
    .await
    .expect("check transactional notification enqueue");
    assert!(queued_before_indexing);

    admin_database
        .index_report_stack_traces(matching_report)
        .await
        .expect("index matching report asynchronously");
    let linked_issue: Option<IssueId> =
        sqlx::query_scalar("SELECT issue_id FROM reports WHERE id = $1")
            .bind(matching_report)
            .fetch_one(&admin_pool)
            .await
            .expect("read automatically linked issue");
    assert_eq!(linked_issue, Some(matching_issue));
    let linked_state: String = sqlx::query_scalar("SELECT state FROM reports WHERE id = $1")
        .bind(matching_report)
        .fetch_one(&admin_pool)
        .await
        .expect("read automatically linked report state");
    assert_eq!(linked_state, "confirmed");
    let queued_after_matching: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM discord_report_notifications WHERE report_id = $1
         )",
    )
    .bind(matching_report)
    .fetch_one(&admin_pool)
    .await
    .expect("check matched report notification suppression");
    assert!(!queued_after_matching);

    admin_database
        .unlink_report_from_issue(matching_issue, matching_report, 999)
        .await
        .expect("intentionally unlink the automatically matched report");
    sqlx::query("UPDATE report_stack_signatures SET algorithm_version = 0 WHERE report_id = $1")
        .bind(matching_report)
        .execute(&admin_pool)
        .await
        .expect("mark unlinked signature for reindexing");
    admin_database
        .index_report_stack_traces(matching_report)
        .await
        .expect("reindex intentionally unlinked report");
    let relinked_issue: Option<IssueId> =
        sqlx::query_scalar("SELECT issue_id FROM reports WHERE id = $1")
            .bind(matching_report)
            .fetch_one(&admin_pool)
            .await
            .expect("check reindexed issue assignment");
    assert_eq!(relinked_issue, None);

    let separate_stack = "#0 abcdef1234567890 0x100 Web::Document::destroy() at /WebContent\n#1 abcdef1234567890 0x200 Web::Page::close() at /WebContent";
    let first_unlinked = insert_stack_report_for_matching(&admin_pool, separate_stack, 7).await;
    let later_unlinked = insert_stack_report_for_matching(&admin_pool, separate_stack, 0).await;
    let recent_duplicate = insert_stack_report_for_matching(&admin_pool, separate_stack, 0).await;
    admin_database
        .index_report_stack_traces(first_unlinked)
        .await
        .expect("index first unlinked report");
    admin_database
        .index_report_stack_traces(later_unlinked)
        .await
        .expect("index report outside the five-minute window");
    admin_database
        .index_report_stack_traces(recent_duplicate)
        .await
        .expect("index recent duplicate");

    let first_queued: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM discord_report_notifications WHERE report_id = $1)",
    )
    .bind(first_unlinked)
    .fetch_one(&admin_pool)
    .await
    .expect("check first unlinked notification");
    let duplicate_queued: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM discord_report_notifications WHERE report_id = $1)",
    )
    .bind(recent_duplicate)
    .fetch_one(&admin_pool)
    .await
    .expect("check recent duplicate notification");
    let later_queued: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM discord_report_notifications WHERE report_id = $1)",
    )
    .bind(later_unlinked)
    .fetch_one(&admin_pool)
    .await
    .expect("check out-of-window notification");
    assert!(first_queued);
    assert!(later_queued);
    assert!(!duplicate_queued);
}

async fn insert_stack_report_for_matching(
    pool: &sqlx::PgPool,
    stack: &str,
    minutes_ago: i32,
) -> ReportId {
    let report_id = ReportId::new();
    sqlx::query(
        "INSERT INTO reports (
            id, submission_id, manifest_digest, kind, client_version,
            build, storage_state, staging_id, created_at
         ) VALUES (
            $1, $2, repeat('d', 64), 'crash', 'matching-test',
            'release', 'ready', $3, now() - make_interval(mins => $4)
         )",
    )
    .bind(report_id)
    .bind(SubmissionId::new())
    .bind(UploadId::new())
    .bind(minutes_ago)
    .execute(pool)
    .await
    .expect("insert matching test report");
    sqlx::query(
        "INSERT INTO report_fields
            (report_id, key, kind, value, recognized_at_submission)
         VALUES ($1, 'stack', 'stack_trace', $2, true)",
    )
    .bind(report_id)
    .bind(serde_json::json!(stack))
    .execute(pool)
    .await
    .expect("insert matching test stack");
    report_id
}

fn github_issue(number: i64, title: &str) -> GithubIssue {
    GithubIssue {
        id: number + 90_000,
        number,
        title: title.into(),
        body: Some("Created while assigning a report.".into()),
        html_url: format!("https://github.com/LadybirdBrowser/ladybird/issues/{number}"),
        state: GithubIssueState::Open,
        updated_at: Utc::now(),
    }
}
