use super::{ReportKind, parse_stack_trace};

const MAX_TITLE_CHARACTERS: usize = 120;
pub const REPORT_TITLE_STACK_CHARACTERS: usize = 8192;

pub struct ReportTitleInput<'a> {
    pub kind: ReportKind,
    pub client_version: &'a str,
    pub failure_reason: Option<&'a str>,
    pub stack_trace: Option<&'a str>,
    pub process: Option<&'a str>,
    pub platform: Option<&'a str>,
    pub signal: Option<&'a str>,
}

pub fn generate_report_title(input: ReportTitleInput<'_>) -> String {
    let kind = match input.kind {
        ReportKind::Crash => "Crash",
        ReportKind::WebCompat => "Web compatibility",
    };

    if input.kind == ReportKind::Crash
        && let Some(location) = input.failure_reason.and_then(source_location_from_failure)
    {
        let title = format!("{kind}: {location}");
        let room = MAX_TITLE_CHARACTERS.saturating_sub(title.chars().count() + " · ".len());

        return match input
            .failure_reason
            .and_then(|reason| failed_expression(reason, room))
        {
            Some(expression) => format!("{title} · {expression}"),
            None => title,
        };
    }

    if let Some(stack) = input.stack_trace {
        let excerpt = stack
            .chars()
            .take(REPORT_TITLE_STACK_CHARACTERS)
            .collect::<String>();
        let parsed = parse_stack_trace(&excerpt);
        if let Some(name) = parsed
            .rows
            .iter()
            .filter(|row| row.relevant)
            .find_map(|row| {
                concise_function_name(&row.symbol, MAX_TITLE_CHARACTERS - kind.len() - 2)
            })
        {
            return format!("{kind}: {name}");
        }
    }

    let mut title = format!("{kind} report");
    let context = [input.process, input.platform]
        .into_iter()
        .flatten()
        .filter_map(|value| safe_context(value, 40))
        .take(2)
        .chain(input.signal.and_then(signal_name))
        .collect::<Vec<_>>();

    if context.is_empty() {
        title.push_str(" · ");
        title.push_str(
            &safe_context(input.client_version, 60).unwrap_or_else(|| "Unknown build".into()),
        );
    } else {
        for value in context {
            title.push_str(" · ");
            title.push_str(&value);
        }
    }

    title
}

/// The condition of a failed verification, such as `a <= b`, or the message of
/// a Rust panic, when it says more than the source location does.
///
/// The text comes from an anonymous client and ends up in titles, including the
/// one proposed for a public issue, so only characters that make up a plain
/// expression are kept; anything else means no expression is shown.
fn failed_expression(reason: &str, room: usize) -> Option<String> {
    let maximum_characters = room.min(60);
    if maximum_characters < 10 {
        return None;
    }

    let (message, _) = reason.rsplit_once(" at ")?;
    let expression = ["Verification failed:", "Rust panic:"]
        .into_iter()
        .find_map(|prefix| message.strip_prefix(prefix))?
        .trim();
    let expression = expression
        .strip_prefix("internal error: entered unreachable code:")
        .unwrap_or(expression)
        .trim();

    if matches!(expression, "" | "false" | "true" | "0" | "1" | "nullptr")
        || !expression.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(
                    character,
                    ' ' | '_'
                        | '.'
                        | ':'
                        | ','
                        | '('
                        | ')'
                        | '['
                        | ']'
                        | '<'
                        | '>'
                        | '='
                        | '!'
                        | '&'
                        | '|'
                        | '+'
                        | '-'
                        | '*'
                        | '/'
                        | '%'
                )
        })
        || expression.contains("//")
    {
        return None;
    }

    if expression.chars().count() <= maximum_characters {
        return Some(expression.to_owned());
    }

    let mut shortened = expression
        .chars()
        .take(maximum_characters - 1)
        .collect::<String>();
    shortened.truncate(shortened.trim_end().len());
    shortened.push('…');
    Some(shortened)
}

fn source_location_from_failure(reason: &str) -> Option<String> {
    let (_, location) = reason.rsplit_once(" at ")?;
    let (path, line) = location.rsplit_once(':')?;

    // Rust panics add the column, as in `file.rs:12:5`. Only the line is kept.
    let (path, line) = match path.rsplit_once(':') {
        Some((path, line_before_column))
            if !line.is_empty()
                && line.bytes().all(|byte| byte.is_ascii_digit())
                && !line_before_column.is_empty()
                && line_before_column.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            (path, line_before_column)
        }
        _ => (path, line),
    };
    if line.parse::<u32>().ok()? == 0 {
        return None;
    }
    let file = path.rsplit('/').next()?;
    if ![".cpp", ".h", ".mm", ".rs", ".c"]
        .iter()
        .any(|extension| file.ends_with(extension))
        || !file.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
        || path.starts_with('/')
        || path.contains("../")
        || path.contains('\\')
    {
        return None;
    }

    Some(format!("{file}:{line}"))
}

pub fn concise_function_name(symbol: &str, maximum_characters: usize) -> Option<String> {
    // AK's callback wrapper often hides the useful function name inside a
    // template argument. Start at that inner function when present.
    let candidate = symbol
        .split_once("::CallableWrapper<")
        .map(|(_, inner)| inner)
        .unwrap_or(symbol);

    // ABI descriptions can precede the actual qualified function name. Find
    // the first scope operator, then discard any words before its qualifier.
    let candidate = if let Some(first_scope) = candidate.find("::") {
        &candidate[candidate[..first_scope]
            .rfind(char::is_whitespace)
            .map_or(0, |index| index + 1)..]
    } else {
        candidate
    };

    let mut without_templates = String::new();
    let mut depth = 0_u32;
    for character in candidate.chars() {
        match character {
            '<' => depth += 1,
            '>' if depth > 0 => depth -= 1,
            _ if depth == 0 => without_templates.push(character),
            _ => {}
        }
    }

    let name = without_templates.split('(').next()?.trim();
    if name.is_empty()
        || name.contains("://")
        || !name
            .chars()
            .all(|character| character.is_alphanumeric() || matches!(character, ':' | '_' | '~'))
    {
        return None;
    }

    let segments = name
        .split("::")
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if segments.is_empty() {
        return None;
    }

    let mut start = segments.len().saturating_sub(3);
    let mut concise = segments[start..].join("::");
    while concise.chars().count() > maximum_characters && start + 1 < segments.len() {
        start += 1;
        concise = segments[start..].join("::");
    }

    if concise.chars().count() > maximum_characters {
        concise = concise.chars().take(maximum_characters - 1).collect();
        concise.push('…');
    }

    Some(concise)
}

/// The name of a signal, without the number some clients put after it, as in
/// `SIGSEGV (11)`.
fn signal_name(signal: &str) -> Option<String> {
    let name = signal.trim().split(['(', ' ']).next()?;

    safe_context(name, 40)
}

fn safe_context(value: &str, maximum_characters: usize) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || !value.chars().all(|character| {
            character.is_alphanumeric()
                || matches!(character, ' ' | '.' | '_' | '-' | '+' | '(' | ')')
        })
    {
        return None;
    }
    Some(value.chars().take(maximum_characters).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_location_takes_priority_over_a_generic_verification_expression() {
        let title = generate_report_title(ReportTitleInput {
            kind: ReportKind::Crash,
            client_version: "1.0",
            failure_reason: Some(
                "Verification failed: false at Libraries/LibMedia/FFmpeg/FFmpegVideoDecoder.cpp:235",
            ),
            stack_trace: Some("#0 0x123 Media::FFmpeg::FFmpegVideoDecoder::take_next_output()"),
            process: Some("WebContent"),
            platform: Some("macOS"),
            signal: None,
        });
        assert_eq!(title, "Crash: FFmpegVideoDecoder.cpp:235");

        let fallback = generate_report_title(ReportTitleInput {
            kind: ReportKind::Crash,
            client_version: "1.0",
            failure_reason: Some("Verification failed: false"),
            stack_trace: Some("#0 0x123 Media::FFmpeg::FFmpegVideoDecoder::take_next_output()"),
            process: Some("WebContent"),
            platform: Some("macOS"),
            signal: None,
        });
        assert_eq!(
            fallback,
            "Crash: FFmpeg::FFmpegVideoDecoder::take_next_output"
        );
    }

    fn title_for_failure(reason: &str) -> String {
        generate_report_title(ReportTitleInput {
            kind: ReportKind::Crash,
            client_version: "1.0",
            failure_reason: Some(reason),
            stack_trace: None,
            process: None,
            platform: None,
            signal: None,
        })
    }

    #[test]
    fn title_adds_the_failed_verification_expression() {
        assert_eq!(
            title_for_failure(
                "Verification failed: m_data.size() <= MAX_MESSAGE_PAYLOAD_SIZE at Libraries/LibIPC/Message.cpp:49"
            ),
            "Crash: Message.cpp:49 · m_data.size() <= MAX_MESSAGE_PAYLOAD_SIZE"
        );

        let long = format!(
            "Verification failed: {} at Libraries/Foo.cpp:1",
            "a_b + ".repeat(20)
        );
        let title = title_for_failure(&long);
        assert!(title.starts_with("Crash: Foo.cpp:1 · a_b + a_b"), "{title}");
        assert!(title.ends_with('…'), "{title}");
        assert!(title.chars().count() <= MAX_TITLE_CHARACTERS);
    }

    #[test]
    fn title_uses_the_location_and_message_of_a_rust_panic() {
        assert_eq!(
            title_for_failure(
                "Rust panic: animation-overlay record is live at Libraries/LibWeb/Rust/src/css/style/computed.rs:3606:14"
            ),
            "Crash: computed.rs:3606 · animation-overlay record is live"
        );
        assert_eq!(
            title_for_failure(
                "Rust panic: internal error: entered unreachable code: the layout update did not stabilize at Libraries/LibWeb/Rust/src/layout/update_layout.rs:638:9"
            ),
            "Crash: update_layout.rs:638 · the layout update did not stabilize"
        );
        assert_eq!(
            title_for_failure("Rust panic: boom at Libraries/Rust/src/lib.rs:7:x"),
            "Crash report · 1.0"
        );
    }

    #[test]
    fn title_stays_within_its_limit_for_a_long_location() {
        let file = format!("{}.cpp", "f".repeat(100));
        let title = title_for_failure(&format!(
            "Verification failed: a_long_condition_name <= another_value at Libraries/{file}:12"
        ));

        assert!(title.chars().count() <= MAX_TITLE_CHARACTERS, "{title}");
        assert!(!title.contains('·'), "{title}");
    }

    #[test]
    fn title_leaves_out_an_expression_that_says_nothing_or_is_not_plain() {
        for reason in [
            "Verification failed: false at Libraries/Foo.cpp:1",
            "Verification failed:  at Libraries/Foo.cpp:1",
            "Verification failed: x == \"https://evil\" at Libraries/Foo.cpp:1",
            "Verification failed: a `b` at Libraries/Foo.cpp:1",
            "Verification failed: see #123 at Libraries/Foo.cpp:1",
            "Verification failed: http://example.test at Libraries/Foo.cpp:1",
            "Something else: a <= b at Libraries/Foo.cpp:1",
        ] {
            assert_eq!(title_for_failure(reason), "Crash: Foo.cpp:1", "{reason}");
        }
    }

    #[test]
    fn title_unwraps_ladybird_callback_symbol() {
        let symbol = "AK::Function<void ()>::CallableWrapper<WebContent::ConnectionFromClient::debug_request(AK::DistinctNumeric<unsigned long long, Web::__PageId_tag, AK::DistinctNumericFeature::Comparison, AK::DistinctNumericFeature::CastToBool>, AK::ByteString, AK::ByteString)::$_2>::call()";
        let stack = format!("#0 abcdef1234567890 0x123 {symbol} at /bin/WebContent");
        let title = generate_report_title(ReportTitleInput {
            kind: ReportKind::Crash,
            client_version: "1.0",
            failure_reason: None,
            stack_trace: Some(&stack),
            process: Some("WebContent"),
            platform: Some("macOS"),
            signal: None,
        });

        assert_eq!(
            title,
            "Crash: WebContent::ConnectionFromClient::debug_request"
        );
        assert!(title.chars().count() <= MAX_TITLE_CHARACTERS);
    }

    #[test]
    fn title_extracts_a_qualified_function_after_abi_description() {
        let stack = "Native stack (binary build ID, object address):\n\
            #0 4402e9f4aa8030998b5a8e8bab39a036 0x100028897 non-virtual thunk to Compositor::ConnectionFromClient::crash() at /bin/Compositor\n\
            #1 4402e9f4aa8030998b5a8e8bab39a036 0x10002b943 CompositorControlServerStub::handle_crash() at /bin/Compositor";
        let title = generate_report_title(ReportTitleInput {
            kind: ReportKind::Crash,
            client_version: "1.0",
            failure_reason: None,
            stack_trace: Some(stack),
            process: Some("Compositor"),
            platform: Some("macOS"),
            signal: None,
        });

        assert_eq!(title, "Crash: Compositor::ConnectionFromClient::crash");
    }

    #[test]
    fn title_falls_back_to_type_and_context() {
        let title = generate_report_title(ReportTitleInput {
            kind: ReportKind::Crash,
            client_version: "1.0",
            failure_reason: None,
            stack_trace: Some("Native stack (binary build ID, object address):"),
            process: Some("WebContent"),
            platform: Some("macOS"),
            signal: None,
        });
        assert_eq!(title, "Crash report · WebContent · macOS");

        let title = generate_report_title(ReportTitleInput {
            kind: ReportKind::WebCompat,
            client_version: "Ladybird Nightly 2026.09.17",
            failure_reason: None,
            stack_trace: None,
            process: None,
            platform: None,
            signal: None,
        });
        assert_eq!(
            title,
            "Web compatibility report · Ladybird Nightly 2026.09.17"
        );
    }

    #[test]
    fn title_rejects_private_or_unusable_symbols() {
        let stack = "#0 0x123 https://private.example/path at WebContent";
        let title = generate_report_title(ReportTitleInput {
            kind: ReportKind::Crash,
            client_version: "1.0",
            failure_reason: None,
            stack_trace: Some(stack),
            process: None,
            platform: Some("macOS"),
            signal: None,
        });
        assert_eq!(title, "Crash report · macOS");
    }

    #[test]
    fn title_bounds_a_single_long_function_name() {
        let function = "very_long_function_name_".repeat(12);
        let stack = format!("#0 0x123 WebContent::{function}(int) at WebContent");
        let title = generate_report_title(ReportTitleInput {
            kind: ReportKind::Crash,
            client_version: "1.0",
            failure_reason: None,
            stack_trace: Some(&stack),
            process: None,
            platform: None,
            signal: None,
        });

        assert!(title.starts_with("Crash: very_long_function_name_"));
        assert!(title.ends_with('…'));
        assert!(title.chars().count() <= MAX_TITLE_CHARACTERS);
    }

    fn title_with_signal(signal: Option<&str>, stack_trace: Option<&str>) -> String {
        generate_report_title(ReportTitleInput {
            kind: ReportKind::Crash,
            client_version: "1.0",
            failure_reason: None,
            stack_trace,
            process: Some("WebContent"),
            platform: Some("macOS"),
            signal,
        })
    }

    #[test]
    fn title_names_the_signal_when_nothing_better_is_known() {
        assert_eq!(
            title_with_signal(Some("SIGTRAP"), None),
            "Crash report · WebContent · macOS · SIGTRAP"
        );
        assert_eq!(
            title_with_signal(
                Some("SIGTRAP"),
                Some("Native stack (binary build ID, object address):\n#0 abcdef1234567890 0x1")
            ),
            "Crash report · WebContent · macOS · SIGTRAP"
        );
        assert_eq!(
            title_with_signal(None, None),
            "Crash report · WebContent · macOS"
        );
    }

    #[test]
    fn title_leaves_the_number_out_of_the_signal() {
        for signal in ["SIGSEGV (11)", "SIGSEGV(11)", "  SIGSEGV  ", "SIGSEGV"] {
            assert_eq!(
                title_with_signal(Some(signal), None),
                "Crash report · WebContent · macOS · SIGSEGV",
                "{signal:?}"
            );
        }
    }

    #[test]
    fn title_ignores_a_signal_that_is_not_a_plain_name() {
        for signal in [
            "",
            " ",
            "(11)",
            "SIG<script>",
            "https://example.test/",
            "SIG/SEGV",
        ] {
            assert_eq!(
                title_with_signal(Some(signal), None),
                "Crash report · WebContent · macOS",
                "{signal:?}"
            );
        }
    }

    #[test]
    fn title_prefers_the_failure_or_a_named_function_over_the_signal() {
        let with_failure = generate_report_title(ReportTitleInput {
            kind: ReportKind::Crash,
            client_version: "1.0",
            failure_reason: Some("Verification failed: false at Libraries/LibMedia/Foo.cpp:235"),
            stack_trace: None,
            process: Some("WebContent"),
            platform: Some("macOS"),
            signal: Some("SIGTRAP"),
        });
        assert_eq!(with_failure, "Crash: Foo.cpp:235");

        assert_eq!(
            title_with_signal(
                Some("SIGTRAP"),
                Some("#0 abcdef1234567890 0x1 Web::Window::close() at /bin/WebContent")
            ),
            "Crash: Web::Window::close"
        );
    }

    #[test]
    fn title_shows_the_signal_without_process_or_platform() {
        let title = generate_report_title(ReportTitleInput {
            kind: ReportKind::Crash,
            client_version: "1.0",
            failure_reason: None,
            stack_trace: None,
            process: None,
            platform: None,
            signal: Some("SIGABRT"),
        });

        assert_eq!(title, "Crash report · SIGABRT");
    }
}
