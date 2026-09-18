use crate::error::{AppError, Result};

use super::report_search::tokenize;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IssueSearch {
    pub terms: Vec<String>,
    pub states: Vec<String>,
    pub github_numbers: Vec<i64>,
    pub ids: Vec<String>,
}

impl IssueSearch {
    pub fn parse(input: &str) -> Result<Self> {
        if input.len() > 512 {
            return Err(AppError::InvalidRequest("Issue search is too long"));
        }

        let tokens = tokenize(input)?;
        if tokens.len() > 32 {
            return Err(AppError::InvalidRequest("Too many issue search terms"));
        }

        let mut search = Self::default();
        for token in tokens {
            let Some((key, value)) = token.split_once(':') else {
                search.terms.push(token);
                continue;
            };

            match key.to_ascii_lowercase().as_str() {
                "state" => {
                    let state = value.to_ascii_lowercase();
                    if !matches!(
                        state.as_str(),
                        "unresolved" | "needs_attention" | "resolved" | "rejected" | "all"
                    ) {
                        return Err(AppError::InvalidRequest("Invalid issue state filter"));
                    }
                    search.states.push(state);
                }
                "github" => {
                    let number = value
                        .trim_start_matches('#')
                        .parse::<i64>()
                        .map_err(|_| AppError::InvalidRequest("Invalid GitHub issue number"))?;
                    if number <= 0 {
                        return Err(AppError::InvalidRequest("Invalid GitHub issue number"));
                    }
                    search.github_numbers.push(number);
                }
                "id" => {
                    if value.is_empty()
                        || !value
                            .bytes()
                            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
                    {
                        return Err(AppError::InvalidRequest("Invalid issue ID filter"));
                    }
                    search.ids.push(value.to_ascii_lowercase());
                }
                _ => return Err(AppError::InvalidRequest("Unknown issue search qualifier")),
            }
        }
        Ok(search)
    }
}

#[cfg(test)]
mod tests {
    use super::IssueSearch;

    #[test]
    fn parses_issue_query_and_repeated_states() {
        let search =
            IssueSearch::parse("renderer state:unresolved state:needs_attention github:#4812")
                .expect("valid issue search");
        assert_eq!(search.terms, ["renderer"]);
        assert_eq!(search.states, ["unresolved", "needs_attention"]);
        assert_eq!(search.github_numbers, [4812]);
    }

    #[test]
    fn rejects_invalid_qualifiers() {
        assert!(IssueSearch::parse("state:missing").is_err());
        assert!(IssueSearch::parse("github:abc").is_err());
        assert!(IssueSearch::parse("platform:linux").is_err());
        assert!(IssueSearch::parse("state:unresolved|resolved").is_err());
    }
}
