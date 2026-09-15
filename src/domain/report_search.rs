use crate::error::{AppError, Result};

const MAXIMUM_SEARCH_BYTES: usize = 512;
const MAXIMUM_SEARCH_TERMS: usize = 32;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReportSearch {
    pub terms: Vec<String>,
    pub qualifiers: Vec<ReportQualifier>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReportQualifier {
    pub key: String,
    pub value: String,
}

impl ReportSearch {
    pub fn parse(input: &str) -> Result<Self> {
        if input.len() > MAXIMUM_SEARCH_BYTES {
            return Err(AppError::InvalidRequest("Report search is too long"));
        }

        let tokens = tokenize(input)?;
        if tokens.len() > MAXIMUM_SEARCH_TERMS {
            return Err(AppError::InvalidRequest("Too many report search terms"));
        }

        let mut search = Self::default();
        for token in tokens {
            if let Some((key, value)) = token.split_once(':') {
                validate_qualifier(key, value)?;
                search.qualifiers.push(ReportQualifier {
                    key: key.to_ascii_lowercase(),
                    value: value.to_owned(),
                });
            } else {
                search.terms.push(token);
            }
        }

        Ok(search)
    }
}

pub fn filter_expression(key: &str, value: &str) -> String {
    if value
        .bytes()
        .all(|byte| !byte.is_ascii_whitespace() && !matches!(byte, b'"' | b'\\'))
    {
        return format!("{key}:{}", value.to_ascii_lowercase());
    }

    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("{key}:\"{escaped}\"")
}

fn tokenize(input: &str) -> Result<Vec<String>> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut quoted = false;
    let mut escaped = false;

    for character in input.chars() {
        if escaped {
            token.push(character);
            escaped = false;
            continue;
        }

        match character {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            character if character.is_whitespace() && !quoted => {
                if !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                }
            }
            _ => token.push(character),
        }
    }

    if quoted || escaped {
        return Err(AppError::InvalidRequest("Unterminated quoted search value"));
    }

    if !token.is_empty() {
        tokens.push(token);
    }

    Ok(tokens)
}

fn validate_qualifier(key: &str, value: &str) -> Result<()> {
    let valid_key = !key.is_empty()
        && key.len() <= 64
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte));

    if !valid_key || value.is_empty() {
        return Err(AppError::InvalidRequest("Invalid report search qualifier"));
    }

    if key.eq_ignore_ascii_case("state")
        && !matches!(
            value.to_ascii_lowercase().as_str(),
            "triage" | "confirmed" | "assigned" | "all"
        )
    {
        return Err(AppError::InvalidRequest("Invalid report state filter"));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_terms_and_qualified_values() {
        let search =
            ReportSearch::parse(r#"navigation platform:linux kind:crash build:"Release ASan""#)
                .expect("valid search");

        assert_eq!(search.terms, ["navigation"]);
        assert_eq!(
            search.qualifiers,
            [
                ReportQualifier {
                    key: "platform".into(),
                    value: "linux".into(),
                },
                ReportQualifier {
                    key: "kind".into(),
                    value: "crash".into(),
                },
                ReportQualifier {
                    key: "build".into(),
                    value: "Release ASan".into(),
                },
            ]
        );
    }

    #[test]
    fn formats_values_for_search_links() {
        assert_eq!(filter_expression("platform", "Linux"), "platform:linux");
        assert_eq!(
            filter_expression("description", "A \"quoted\" value"),
            r#"description:"A \"quoted\" value""#
        );
    }

    #[test]
    fn validates_report_states() {
        assert!(ReportSearch::parse("state:triage").is_ok());
        assert!(ReportSearch::parse("state:confirmed").is_ok());
        assert!(ReportSearch::parse("state:assigned").is_ok());
        assert!(ReportSearch::parse("state:triage state:confirmed").is_ok());
        assert!(ReportSearch::parse("state:all").is_ok());
        assert!(ReportSearch::parse("state:triage|confirmed").is_err());
        assert!(ReportSearch::parse("state:unknown").is_err());
    }

    #[test]
    fn rejects_unterminated_quotes() {
        assert!(ReportSearch::parse(r#"platform:"linux"#).is_err());
    }
}
