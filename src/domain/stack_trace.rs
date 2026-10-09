use sha2::{Digest, Sha256};

use super::ReportKind;

/// Version 2 leaves crash and panic plumbing out of the frames, names frames the
/// same way whatever the client's symbol format, and includes the failure reason.
pub const STACK_SIGNATURE_VERSION: i32 = 2;

const MAX_RENDERED_LINES: usize = 256;
const MAX_SIGNATURE_FRAMES: usize = 24;
const SIGNATURE_FRAMES: usize = 5;
const SIGNATURE_ADDRESSES: usize = 3;
const NATIVE_STACK_HEADER: &str = "Native stack (binary build ID, object address):";

/// A presentation of a submitted stack. The original text remains in report_fields.
pub struct ParsedStackTrace {
    pub rows: Vec<StackTraceRow>,
    pub frame_count: usize,
    /// The names of the functions in the stack, without the plumbing that ends a
    /// crash, for the signature.
    pub frame_keys: Vec<String>,
    /// Build ID and address of frames that have no symbol, for a signature when
    /// nothing else identifies the stack.
    pub address_keys: Vec<String>,
    pub truncated: bool,
}

pub struct StackTraceRow {
    pub number: Option<u32>,
    pub top: bool,
    pub symbol: String,
    pub module: String,
    pub address: String,
    pub build_id: String,
    pub raw: String,
    pub relevant: bool,
    /// The frame has no symbol: the client sent an address, or nothing at all.
    pub unavailable: bool,
}

pub fn parse_stack_trace(text: &str) -> ParsedStackTrace {
    let mut rows = Vec::new();
    let mut frame_keys = Vec::new();
    let mut address_keys = Vec::new();
    let mut frame_count = 0;
    let mut truncated = false;

    for (index, line) in text.lines().enumerate() {
        if index >= MAX_RENDERED_LINES {
            truncated = true;
            break;
        }

        // Ladybird prefixes native stacks with a format label. Keep it in the
        // original text, but do not render it as an unparsed stack row.
        if index == 0 && line.trim() == NATIVE_STACK_HEADER {
            continue;
        }

        let row = parse_frame(line).unwrap_or_else(|| StackTraceRow {
            number: None,
            top: false,
            symbol: String::new(),
            module: String::new(),
            address: String::new(),
            build_id: String::new(),
            raw: line.to_owned(),
            relevant: false,
            unavailable: false,
        });

        if row.number.is_some() {
            frame_count += 1;
        }
        if row.relevant
            && frame_keys.len() < MAX_SIGNATURE_FRAMES
            && let Some(name) = function_name(&row.symbol)
        {
            frame_keys.push(name);
        }
        if row.unavailable && !row.build_id.is_empty() && address_keys.len() < SIGNATURE_ADDRESSES {
            address_keys.push(format!("{}:{}", row.build_id, row.address));
        }
        rows.push(row);
    }

    ParsedStackTrace {
        rows,
        frame_count,
        frame_keys,
        address_keys,
        truncated,
    }
}

/// Identifies the crash a stack belongs to. A failure reason that says where and
/// why the process stopped is the best evidence there is, so it is part of the
/// identity; without frames it can stand alone. Stacks that were not symbolicated
/// are told apart by the addresses in them.
pub fn stack_fingerprint(
    kind: ReportKind,
    process: Option<&str>,
    signal: Option<&str>,
    failure_reason: Option<&str>,
    parsed: &ParsedStackTrace,
) -> Option<String> {
    let failure = failure_reason.and_then(failure_key);
    let frames = if failure.is_some() || !parsed.frame_keys.is_empty() {
        &parsed.frame_keys
    } else {
        &parsed.address_keys
    };
    if frames.is_empty() && failure.is_none() {
        return None;
    }

    let process = process.unwrap_or("").trim().to_ascii_lowercase();
    let signal = signal
        .unwrap_or("")
        .split(['(', ' '])
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();

    let mut digest = Sha256::new();
    for component in [
        kind.as_str(),
        &process,
        &signal,
        &failure.unwrap_or_default(),
    ]
    .into_iter()
    .chain(frames.iter().take(SIGNATURE_FRAMES).map(String::as_str))
    {
        digest.update(component.as_bytes());
        digest.update([0]);
    }
    Some(hex::encode(digest.finalize()))
}

/// What a failure reason says, without the numbers that change from one build or
/// run to the next: the message, with its digits blanked, and the source file but
/// not the line, as in `assertion failed: index < len@src/lib.rs`.
fn failure_key(reason: &str) -> Option<String> {
    let reason = reason.trim();
    let (message, location) = match reason.rsplit_once(" at ") {
        Some((message, location)) if location.contains(':') => (message, location),
        _ => (reason, ""),
    };
    let file = location.split(':').next().unwrap_or_default();
    if message.is_empty() {
        return None;
    }

    let mut key = String::new();
    for character in message.to_lowercase().chars() {
        if character.is_ascii_digit() {
            if !key.ends_with('#') {
                key.push('#');
            }
        } else {
            key.push(character);
        }
    }

    Some(format!("{key}@{file}"))
}

fn parse_frame(line: &str) -> Option<StackTraceRow> {
    let rest = line.trim_start().strip_prefix('#')?;
    let number_length = rest.bytes().take_while(u8::is_ascii_digit).count();
    let number = rest.get(..number_length)?.parse().ok()?;
    let mut rest = rest.get(number_length..)?.trim_start();

    if rest.eq_ignore_ascii_case("unavailable") {
        return Some(unavailable_frame(number, "—".into(), String::new()));
    }

    let first = rest.split_whitespace().next()?;
    let build_id = if first.len() >= 16 && first.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        rest = rest.get(first.len()..)?.trim_start();
        first.to_owned()
    } else {
        String::new()
    };

    let address = rest.split_whitespace().next()?;
    if !address.starts_with("0x") || !address[2..].bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    rest = rest.get(address.len()..)?.trim_start();
    if rest.is_empty() {
        return Some(unavailable_frame(number, address.to_owned(), build_id));
    }
    let (symbol, module) = match rest.rsplit_once(" at ") {
        Some((symbol, module)) => (symbol.trim(), module.trim()),
        None => (rest.trim(), ""),
    };
    if symbol.is_empty() {
        return None;
    }

    let module = module.rsplit(['/', '\\']).next().unwrap_or(module);
    Some(StackTraceRow {
        number: Some(number),
        top: number == 0,
        symbol: symbol.to_owned(),
        module: module.to_owned(),
        address: address.to_owned(),
        build_id,
        raw: String::new(),
        relevant: !is_generic_frame(symbol) && !is_runtime_frame(symbol),
        unavailable: false,
    })
}

fn unavailable_frame(number: u32, address: String, build_id: String) -> StackTraceRow {
    StackTraceRow {
        number: Some(number),
        top: number == 0,
        symbol: "Unavailable".into(),
        module: "—".into(),
        address,
        build_id,
        raw: String::new(),
        relevant: false,
        unavailable: true,
    }
}

/// A symbol as written by the client, without what a build or the symbol format
/// adds: the offset into the function and the marker of an inlined call.
fn bare_symbol(symbol: &str) -> &str {
    let symbol = match symbol.rsplit_once(" + ") {
        Some((head, offset))
            if offset.bytes().all(|byte| byte.is_ascii_digit())
                || offset
                    .strip_prefix("0x")
                    .is_some_and(|hex| hex.bytes().all(|byte| byte.is_ascii_hexdigit())) =>
        {
            head
        }
        _ => symbol,
    };

    symbol
        .trim()
        .strip_prefix("(inlined) ")
        .unwrap_or(symbol)
        .trim()
}

/// The name of the function a frame is in, without its namespace, parameters,
/// template arguments or return type, so that clients that write symbols
/// differently, or compilers that inline differently, agree on it. Frames with
/// nothing to name and lambdas have none.
fn function_name(symbol: &str) -> Option<String> {
    let symbol = bare_symbol(symbol);
    if symbol.is_empty() || symbol.starts_with("at ") || symbol == "Unavailable" {
        return None;
    }

    let symbol = demangled(symbol);
    let mut plain = String::new();
    let mut depth = 0_u32;
    for character in symbol.chars() {
        match character {
            '<' => depth += 1,
            '>' if depth > 0 => depth -= 1,
            _ if depth == 0 => plain.push(character),
            _ => {}
        }
    }

    // `operator()` and its relatives would be cut apart by the parameter list.
    if plain.contains("operator") {
        return None;
    }

    let name = plain
        .split('(')
        .next()?
        .trim()
        .rsplit(' ')
        .next()?
        .rsplit("::")
        .find(|segment| !segment.is_empty() && !segment.starts_with('{'))?;
    let valid = name
        .chars()
        .all(|character| character.is_alphanumeric() || matches!(character, '_' | '$' | '~' | '.'));

    (valid && name.len() <= 128).then(|| name.to_owned())
}

/// The path of a Rust symbol in v0 mangling, such as `std::process::abort`.
/// Other symbols are returned as they are. Only the names are recovered, which
/// is all a signature needs; crate hashes and generic arguments are dropped.
fn demangled(symbol: &str) -> String {
    if !symbol.starts_with("_R") {
        return symbol.to_owned();
    }

    let bytes = symbol.as_bytes();
    let mut names = Vec::new();
    let mut index = 2;
    while index < bytes.len() {
        match bytes[index] {
            // A crate's disambiguator, `s<base 62>_`, or a back reference, `B<base 62>_`.
            b'C' if bytes.get(index + 1) == Some(&b's') => {
                index += symbol[index..].find('_').map_or(bytes.len(), |end| end + 1);
            }
            b'B' => index += symbol[index..].find('_').map_or(bytes.len(), |end| end + 1),
            b'0'..=b'9' => {
                let digits = bytes[index..]
                    .iter()
                    .take_while(|byte| byte.is_ascii_digit())
                    .count();
                let length = symbol[index..index + digits].parse::<usize>().unwrap_or(0);
                // A `_` separates the length from a name that starts with `_`.
                let start = index + digits + usize::from(bytes.get(index + digits) == Some(&b'_'));
                match symbol.get(start..start + length) {
                    Some(name) if length > 0 => {
                        names.push(name);
                        index = start + length;
                    }
                    _ => index = start,
                }
            }
            _ => index += 1,
        }
    }

    names.join("::")
}

/// Frames of the machinery that ends a process, which say how it stopped but not
/// why: raising the signal, aborting, the panic runtime and thread start-up.
fn is_runtime_frame(symbol: &str) -> bool {
    const FUNCTIONS: &[&str] = &[
        "__pthread_kill",
        "pthread_kill",
        "raise",
        "gsignal",
        "abort",
        "__assert_rtn",
        "__assert_fail",
        "ak_trap",
        "__builtin_trap",
        "_thread_start",
        "__pthread_start",
        "start_thread",
        "thread_start",
    ];
    const RUST_PATHS: &[&str] = &[
        "std::panicking",
        "core::panicking",
        "std::process::abort",
        "std::sys::backtrace",
        "std::sys::pal::unix::abort_internal",
        "__rustc::",
        "rust_begin_unwind",
        "core::option::expect_failed",
        "core::option::unwrap_failed",
        "core::result::unwrap_failed",
    ];

    let symbol = demangled(bare_symbol(symbol));

    FUNCTIONS.contains(&symbol.as_str()) || RUST_PATHS.iter().any(|path| symbol.contains(path))
}

/// Frames of the program's event loop and entry points, which every stack of a
/// thread passes through and so say nothing about the crash.
fn is_generic_frame(symbol: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "Core::ThreadEventQueue::process",
        "Core::EventLoopImplementationUnix::exec",
        "Core::Timer::timer_event",
        "ladybird_main(",
    ];

    let symbol = bare_symbol(symbol);

    matches!(symbol, "_main" | "main" | "_start")
        || PREFIXES.iter().any(|prefix| symbol.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ladybird_frames_and_keeps_unparsed_lines() {
        let text = include_str!("../../tests/fixtures/webcontent-stack.txt");
        let parsed = parse_stack_trace(text);
        assert_eq!(parsed.rows[0].number, Some(0));
        assert_eq!(parsed.rows[0].module, "WebContent");
        assert!(parsed.rows[0].symbol.contains("debug_request"));
        assert_eq!(parsed.rows[1].address, "0x1f3c3");
        assert_eq!(parsed.rows[5].number, Some(5));
        assert_eq!(parsed.rows[5].symbol, "Unavailable");
        assert_eq!(parsed.frame_keys.len(), 1);
    }

    #[test]
    fn fingerprint_ignores_addresses_and_build_paths() {
        let first = parse_stack_trace(
            "#0 abcdef1234567890 0x123 Web::Window::close() at /a/WebContent\n\
             #1 abcdef1234567890 0x456 Web::Page::destroy() at /a/WebContent",
        );
        let second = parse_stack_trace(
            "#0 1234567890abcdef 0x999 Web::Window::close() at /b/WebContent\n\
             #1 1234567890abcdef 0xaaa Web::Page::destroy() at /b/WebContent",
        );
        let sign =
            |parsed, signal| stack_fingerprint(ReportKind::Crash, None, Some(signal), None, parsed);

        assert_eq!(first.frame_keys, ["close", "destroy"]);
        assert_eq!(first.frame_keys, second.frame_keys);
        assert_eq!(sign(&first, "SIGSEGV"), sign(&second, "SIGSEGV"));
        assert_ne!(sign(&first, "SIGSEGV"), sign(&second, "SIGABRT"));
    }

    #[test]
    fn parses_numbered_frames_without_symbols() {
        let parsed = parse_stack_trace(
            "#58 07de8215b79e3ecbb77386b53a7117e7 0x24523 Core::EventLoopImplementationUnix::exec() + 0x57\n\
             #59 3c66d912e69f3437bc0fbb00f7d342e9 0x10000a2c7\n\
             #60 3c66d912e69f3437bc0fbb00f7d342e9 0x100124f3f\n\
             #61 unavailable",
        );

        assert_eq!(parsed.frame_count, 4);
        for (number, address) in [(59, "0x10000a2c7"), (60, "0x100124f3f")] {
            let row = &parsed.rows[(number - 58) as usize];
            assert_eq!(row.number, Some(number));
            assert_eq!(row.symbol, "Unavailable");
            assert_eq!(row.address, address);
            assert_eq!(row.build_id, "3c66d912e69f3437bc0fbb00f7d342e9");
            assert!(!row.relevant);
        }
    }

    // Real stacks from reports, trimmed to their first lines. They are the
    // regressions signature version 2 was made to fix.
    #[derive(serde::Deserialize)]
    struct Fixture {
        name: String,
        kind: String,
        process: Option<String>,
        signal: Option<String>,
        failure_reason: Option<String>,
        stack: String,
    }

    fn fixture(name: &str) -> Fixture {
        serde_json::from_str::<Vec<Fixture>>(include_str!(
            "../../tests/fixtures/stack-signatures.json"
        ))
        .expect("fixtures are valid JSON")
        .into_iter()
        .find(|fixture| fixture.name == name)
        .unwrap_or_else(|| panic!("no fixture named {name}"))
    }

    fn signature(name: &str) -> Option<String> {
        let fixture = fixture(name);

        stack_fingerprint(
            ReportKind::parse(&fixture.kind).expect("a report kind"),
            fixture.process.as_deref(),
            fixture.signal.as_deref(),
            fixture
                .failure_reason
                .as_deref()
                .filter(|reason| !reason.is_empty()),
            &parse_stack_trace(&fixture.stack),
        )
    }

    fn assert_same_crash(names: &[&str]) {
        let signatures = names.iter().map(|name| signature(name)).collect::<Vec<_>>();

        assert!(signatures[0].is_some(), "{names:?} have a signature");
        assert!(
            signatures.windows(2).all(|pair| pair[0] == pair[1]),
            "{names:?} should share a signature: {signatures:?}"
        );
    }

    fn assert_different_crashes(names: &[&str]) {
        let signatures = names.iter().map(|name| signature(name)).collect::<Vec<_>>();

        for (index, first) in signatures.iter().enumerate() {
            assert!(first.is_some(), "{} has a signature", names[index]);
            for (other, second) in signatures.iter().enumerate().skip(index + 1) {
                assert_ne!(first, second, "{} and {}", names[index], names[other]);
            }
        }
    }

    #[test]
    fn panics_are_told_apart_by_what_they_say() {
        // All of these stop in the same panic and abort machinery, which was
        // all the first version of the signature looked at.
        assert_different_crashes(&[
            "panic_tree_builder_2273",
            "panic_computed_3606",
            "panic_mod_1133_macos",
            "panic_update_layout_1069_macos",
        ]);
    }

    #[test]
    fn a_failure_with_the_same_message_in_the_same_file_is_the_same_crash() {
        // The crash site is not in the stack, and the event loop around it varies.
        assert_same_crash(&[
            "panic_computed_3606",
            "panic_computed_3606_other_event_loop_path",
        ]);

        // Different builds, and a source line that moved between them.
        assert_same_crash(&["panic_mod_1133_linux", "panic_mod_1133_linux_other_build"]);
        assert_same_crash(&[
            "panic_update_layout_1069_macos",
            "panic_update_layout_1077_macos",
            "panic_update_layout_1069_linux",
        ]);
    }

    #[test]
    fn clients_that_write_symbols_differently_agree() {
        assert_same_crash(&[
            "webgl_connection_old_symbols",
            "webgl_connection_new_symbols",
        ]);
    }

    #[test]
    fn the_same_stack_with_another_failure_is_another_crash() {
        assert_different_crashes(&["verify_node_3862", "verify_document_observer_46"]);
    }

    #[test]
    fn stacks_without_symbols_are_told_apart_by_their_addresses() {
        assert_same_crash(&["addresses_only_a", "addresses_only_b", "addresses_only_c"]);
        assert_different_crashes(&["addresses_only_a", "addresses_only_other_build"]);
    }

    #[test]
    fn a_stack_with_nothing_in_it_has_no_signature() {
        assert_eq!(signature("nothing_available"), None);
    }

    #[test]
    fn a_failure_reason_alone_is_enough_for_a_signature() {
        let empty =
            parse_stack_trace("Native stack (binary build ID, object address):\n#0 unavailable");
        let sign = |reason| {
            stack_fingerprint(
                ReportKind::Crash,
                Some("WebContent"),
                Some("SIGABRT"),
                reason,
                &empty,
            )
        };

        assert!(sign(Some("Verification failed: x at Libraries/A.cpp:1")).is_some());
        assert_ne!(
            sign(Some("Verification failed: x at Libraries/A.cpp:1")),
            sign(Some("Verification failed: y at Libraries/A.cpp:1"))
        );
        assert_eq!(sign(None), None);
    }

    #[test]
    fn the_signal_number_is_not_part_of_the_signature() {
        let parsed = parse_stack_trace("#0 0x1 Web::Window::close()");
        let sign = |signal| stack_fingerprint(ReportKind::Crash, None, Some(signal), None, &parsed);

        assert_eq!(sign("SIGTRAP (5)"), sign("SIGTRAP"));
        assert_eq!(sign("sigtrap"), sign("SIGTRAP"));
    }

    #[test]
    fn the_machinery_that_ends_a_process_is_not_part_of_the_stack() {
        let parsed = parse_stack_trace(&fixture("panic_tree_builder_2273").stack);

        assert!(parsed.frame_keys.is_empty(), "{:?}", parsed.frame_keys);
        assert!(
            parsed
                .rows
                .iter()
                .filter(|row| row.number.is_some() && !row.unavailable)
                .all(|row| !row.relevant),
            "panic and abort frames are runtime frames"
        );

        let parsed = parse_stack_trace(&fixture("panic_mod_1133_linux").stack);
        assert!(parsed.frame_keys.is_empty(), "{:?}", parsed.frame_keys);
    }

    #[test]
    fn functions_are_named_the_same_whatever_the_symbol_format() {
        for (symbol, name) in [
            (
                "Web::HTML::HTMLCanvasElement::get_context(AK::Utf16View, JS::Value)",
                "get_context",
            ),
            ("get_context", "get_context"),
            ("get_context + 0x1f", "get_context"),
            ("get_context + 135", "get_context"),
            ("(inlined) get_context", "get_context"),
            (
                "(inlined) AK::NonnullOwnPtr<Messages::Response> IPC::Connection::send_sync<Messages::Create, int &>(int &)",
                "send_sync",
            ),
            (
                "JS::ThrowCompletionOr<Web::HTML::Has> Web::HTML::Canvas::get_context(JS::VM&)",
                "get_context",
            ),
            ("Web::Page::~Page()", "~Page"),
            ("Threading::Thread::start()::$_0::__invoke(void*)", "start"),
            ("_RNvNtCs7mRY9FNn263_3std7process5abort + 0xb", "abort"),
            ("_RNvCsabc123_4core9panicking9panic_fmt", "panic_fmt"),
        ] {
            assert_eq!(function_name(symbol).as_deref(), Some(name), "{symbol}");
        }

        for symbol in [
            "Unavailable",
            "at Build/release/lib/liblagom-web.dylib",
            "(inlined) operator()",
            "AK::Function<void ()>::operator()() const",
            "operator<(int)",
            "",
        ] {
            assert_eq!(function_name(symbol), None, "{symbol}");
        }
    }

    #[test]
    fn runtime_frames_are_recognised() {
        for symbol in [
            "__pthread_kill + 0x8",
            "pthread_kill + 0x11c",
            "gsignal + 0x1d",
            "abort + 0x93",
            "_RNvNtCs7mRY9FNn263_3std7process5abort + 0xb",
            "_RNvNtCslWxY2MhVcag_4core9panicking9panic_fmt + 0x27",
            "std::sys::pal::unix::abort_internal",
            "std::panicking::panic_handler::{closure#0}",
            "__rustc::rust_panic",
            "core::option::expect_failed",
        ] {
            assert!(is_runtime_frame(symbol), "{symbol}");
        }

        for symbol in [
            "Web::Page::abort_navigation()",
            "Core::Timer::timer_event(Core::TimerEvent&)",
            "Web::Layout::update_layout",
        ] {
            assert!(!is_runtime_frame(symbol), "{symbol}");
        }
    }

    #[test]
    fn failure_keys_ignore_numbers_and_lines() {
        assert_eq!(
            failure_key("Verification failed: a <= b at Libraries/LibIPC/Message.cpp:49")
                .as_deref(),
            Some("verification failed: a <= b@Libraries/LibIPC/Message.cpp")
        );
        assert_eq!(
            failure_key("Rust panic: index 7 out of range for 12 at Libraries/A/src/b.rs:3:9"),
            failure_key("Rust panic: index 8 out of range for 99 at Libraries/A/src/b.rs:40:2")
        );
        assert_ne!(
            failure_key("Rust panic: x at Libraries/A/src/b.rs:3:9"),
            failure_key("Rust panic: x at Libraries/A/src/c.rs:3:9")
        );
        assert_eq!(failure_key("  "), None);
    }
}
