use sha2::{Digest, Sha256};

pub const STACK_SIGNATURE_VERSION: i32 = 1;
const MAX_RENDERED_LINES: usize = 256;
const MAX_SIGNATURE_FRAMES: usize = 24;
const NATIVE_STACK_HEADER: &str = "Native stack (binary build ID, object address):";

/// A presentation of a submitted stack. The original text remains in report_fields.
pub struct ParsedStackTrace {
    pub rows: Vec<StackTraceRow>,
    pub frame_count: usize,
    pub frame_keys: Vec<String>,
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
}

pub fn parse_stack_trace(text: &str) -> ParsedStackTrace {
    let mut rows = Vec::new();
    let mut frame_keys = Vec::new();
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
        });

        if row.number.is_some() {
            frame_count += 1;
        }
        if row.relevant && frame_keys.len() < MAX_SIGNATURE_FRAMES {
            let normalized = normalize_symbol(&row.symbol);
            if normalized.len() <= 512 {
                frame_keys.push(normalized);
            }
        }
        rows.push(row);
    }

    ParsedStackTrace {
        rows,
        frame_count,
        frame_keys,
        truncated,
    }
}

pub fn stack_fingerprint(
    kind: &str,
    process: Option<&str>,
    signal: Option<&str>,
    frame_keys: &[String],
) -> Option<String> {
    if frame_keys.is_empty() {
        return None;
    }

    let mut digest = Sha256::new();
    let process = process.unwrap_or("").trim().to_ascii_lowercase();
    let signal = signal.unwrap_or("").trim().to_ascii_uppercase();
    for component in [kind, &process, &signal]
        .into_iter()
        .chain(frame_keys.iter().take(5).map(String::as_str))
    {
        digest.update(component.as_bytes());
        digest.update([0]);
    }
    Some(hex::encode(digest.finalize()))
}

fn parse_frame(line: &str) -> Option<StackTraceRow> {
    let rest = line.trim_start().strip_prefix('#')?;
    let number_length = rest.bytes().take_while(u8::is_ascii_digit).count();
    let number = rest.get(..number_length)?.parse().ok()?;
    let mut rest = rest.get(number_length..)?.trim_start();

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
        relevant: !is_generic_frame(symbol),
    })
}

fn normalize_symbol(symbol: &str) -> String {
    symbol.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_generic_frame(symbol: &str) -> bool {
    matches!(
        symbol,
        "Core::ThreadEventQueue::process()"
            | "Core::EventLoopImplementationUnix::exec()"
            | "ladybird_main(Main::Arguments)"
            | "_main"
            | "main"
            | "_start"
    )
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
        assert_eq!(parsed.rows[5].raw, "#5 unavailable");
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
        assert_eq!(first.frame_keys, second.frame_keys);
        assert_eq!(
            stack_fingerprint("crash", None, Some("SIGSEGV"), &first.frame_keys),
            stack_fingerprint("crash", None, Some("SIGSEGV"), &second.frame_keys)
        );
        assert_ne!(
            stack_fingerprint("crash", None, Some("SIGSEGV"), &first.frame_keys),
            stack_fingerprint("crash", None, Some("SIGABRT"), &second.frame_keys)
        );
    }
}
