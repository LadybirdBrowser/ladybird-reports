use super::parse_stack_trace;

const MAX_TITLE_CHARACTERS: usize = 120;
pub const REPORT_TITLE_STACK_CHARACTERS: usize = 8192;

pub struct ReportTitleInput<'a> {
    pub kind: &'a str,
    pub client_version: &'a str,
    pub stack_trace: Option<&'a str>,
    pub process: Option<&'a str>,
    pub platform: Option<&'a str>,
}

pub fn generate_report_title(input: ReportTitleInput<'_>) -> String {
    let kind = match input.kind {
        "crash" => "Crash",
        "web_compat" => "Web compatibility",
        _ => "Diagnostic",
    };

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
    fn title_unwraps_ladybird_callback_symbol() {
        let symbol = "AK::Function<void ()>::CallableWrapper<WebContent::ConnectionFromClient::debug_request(AK::DistinctNumeric<unsigned long long, Web::__PageId_tag, AK::DistinctNumericFeature::Comparison, AK::DistinctNumericFeature::CastToBool>, AK::ByteString, AK::ByteString)::$_2>::call()";
        let stack = format!("#0 abcdef1234567890 0x123 {symbol} at /bin/WebContent");
        let title = generate_report_title(ReportTitleInput {
            kind: "crash",
            client_version: "1.0",
            stack_trace: Some(&stack),
            process: Some("WebContent"),
            platform: Some("macOS"),
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
            kind: "crash",
            client_version: "1.0",
            stack_trace: Some(stack),
            process: Some("Compositor"),
            platform: Some("macOS"),
        });

        assert_eq!(title, "Crash: Compositor::ConnectionFromClient::crash");
    }

    #[test]
    fn title_falls_back_to_type_and_context() {
        let title = generate_report_title(ReportTitleInput {
            kind: "crash",
            client_version: "1.0",
            stack_trace: Some("Native stack (binary build ID, object address):"),
            process: Some("WebContent"),
            platform: Some("macOS"),
        });
        assert_eq!(title, "Crash report · WebContent · macOS");

        let title = generate_report_title(ReportTitleInput {
            kind: "web_compat",
            client_version: "Ladybird Nightly 2026.09.17",
            stack_trace: None,
            process: None,
            platform: None,
        });
        assert_eq!(
            title,
            "Web compatibility report · Ladybird Nightly 2026.09.17"
        );

        let title = generate_report_title(ReportTitleInput {
            kind: "future_kind",
            client_version: "1.0",
            stack_trace: None,
            process: None,
            platform: None,
        });
        assert_eq!(title, "Diagnostic report · 1.0");
    }

    #[test]
    fn title_rejects_private_or_unusable_symbols() {
        let stack = "#0 0x123 https://private.example/path at WebContent";
        let title = generate_report_title(ReportTitleInput {
            kind: "crash",
            client_version: "1.0",
            stack_trace: Some(stack),
            process: None,
            platform: Some("macOS"),
        });
        assert_eq!(title, "Crash report · macOS");
    }

    #[test]
    fn title_bounds_a_single_long_function_name() {
        let function = "very_long_function_name_".repeat(12);
        let stack = format!("#0 0x123 WebContent::{function}(int) at WebContent");
        let title = generate_report_title(ReportTitleInput {
            kind: "crash",
            client_version: "1.0",
            stack_trace: Some(&stack),
            process: None,
            platform: None,
        });

        assert!(title.starts_with("Crash: very_long_function_name_"));
        assert!(title.ends_with('…'));
        assert!(title.chars().count() <= MAX_TITLE_CHARACTERS);
    }
}
