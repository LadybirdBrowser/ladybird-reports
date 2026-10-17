//! A bar of quick filters above a list. Each pill stands for one search qualifier, such as `state:triage`, and
//! toggles it in the search box of the list. The bar is rendered by `templates/_filter_bar.html` and driven by
//! `assets/filter-bar.js`; a page uses it by putting a [`FilterBar`] in its template and loading the script.
//!
//! What a click does is defined in `assets/filter-bar.js`, and the link of every pill is what a plain click does
//! without it, so [`plain_click_query`] and the script must agree:
//!
//! - a plain click on a pill that is not active replaces the query with its qualifier;
//! - a plain click on an active pill, when others of its kind are active too, keeps only that one;
//! - a plain click on the only active pill of its kind removes it and puts the default ones back, leaving the rest of
//!   the query alone;
//! - with Cmd or Ctrl, a pill that is not active adds its qualifier to the query, and an active one removes it.

/// One pill: a qualifier, how many items it matches, and whether the current query contains it.
pub struct FilterPill {
    pub label: &'static str,
    /// Names the color of the pill, as in `filter-pill-<tone>`.
    pub tone: &'static str,
    pub count: Option<i64>,
    /// The qualifier, such as `state:triage`.
    pub expression: String,
    pub active: bool,
    /// Where a plain click leads when the script is not running.
    pub href: String,
}

/// What a pill is made from.
pub struct FilterOption {
    pub label: &'static str,
    pub tone: &'static str,
    pub count: Option<i64>,
    /// The qualifier, such as `state:triage`.
    pub expression: String,
}

pub struct FilterBar {
    /// The accessible name of the bar.
    pub label: &'static str,
    /// The query a list starts with, which a plain click on an active pill returns to.
    pub default_query: String,
    pub pills: Vec<FilterPill>,
}

impl FilterBar {
    /// A bar for the list at `path`, showing which of `options` the `query` contains.
    pub fn new(
        label: &'static str,
        path: &str,
        default_query: &str,
        query: &str,
        options: impl IntoIterator<Item = FilterOption>,
    ) -> Self {
        let pills = options
            .into_iter()
            .map(|option| {
                let active = contains(query, &option.expression);
                FilterPill {
                    label: option.label,
                    tone: option.tone,
                    count: option.count,
                    href: link(
                        path,
                        &plain_click_query(query, default_query, &option.expression),
                    ),
                    expression: option.expression,
                    active,
                }
            })
            .collect();

        Self {
            label,
            default_query: default_query.to_owned(),
            pills,
        }
    }
}

/// The query after a plain click on the pill of `expression`.
pub fn plain_click_query(query: &str, default_query: &str, expression: &str) -> String {
    let Some((key, _)) = qualifier(expression) else {
        return expression.to_owned();
    };

    if !contains(query, expression) {
        return expression.to_owned();
    }

    let wanted = qualifier(expression);
    let of_kind = |token: &str| qualifier(token).is_some_and(|(token_key, _)| token_key == key);

    let mut kept: Vec<&str> = tokens(query)
        .into_iter()
        .filter(|token| !of_kind(token))
        .collect();
    let others_of_kind = tokens(query)
        .into_iter()
        .any(|token| of_kind(token) && qualifier(token) != wanted);

    if others_of_kind {
        kept.push(expression);
    } else {
        kept.extend(
            tokens(default_query)
                .into_iter()
                .filter(|token| of_kind(token)),
        );
    }
    kept.join(" ")
}

fn contains(query: &str, expression: &str) -> bool {
    let Some(wanted) = qualifier(expression) else {
        return false;
    };
    tokens(query)
        .into_iter()
        .any(|token| qualifier(token).is_some_and(|found| found == wanted))
}

fn link(path: &str, query: &str) -> String {
    let mut url = reqwest::Url::parse("http://localhost/").expect("static URL is valid");
    url.query_pairs_mut().append_pair("q", query);
    format!("{path}?{}", url.query().unwrap_or_default())
}

/// The words of a query, with quoted values kept whole and as written.
fn tokens(query: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start = None;
    let mut quoted = false;
    let mut escaped = false;

    for (index, character) in query.char_indices() {
        if character.is_whitespace() && !quoted {
            if let Some(start) = start.take() {
                tokens.push(&query[start..index]);
            }
            continue;
        }

        start.get_or_insert(index);
        match character {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            _ => {}
        }
    }

    if let Some(start) = start {
        tokens.push(&query[start..]);
    }
    tokens
}

/// The key and value of a qualifier, lowercase and without the quotes around the value.
fn qualifier(token: &str) -> Option<(String, String)> {
    let (key, value) = token.split_once(':')?;
    let value = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value);
    Some((key.to_ascii_lowercase(), value.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::{FilterBar, FilterOption, plain_click_query, tokens};

    const DEFAULT: &str = "state:triage state:confirmed";

    fn option(expression: &str) -> FilterOption {
        FilterOption {
            label: "Option",
            tone: "neutral",
            count: None,
            expression: expression.into(),
        }
    }

    #[test]
    fn the_default_query_activates_the_pills_it_contains() {
        let bar = FilterBar::new(
            "Status",
            "/",
            DEFAULT,
            DEFAULT,
            ["state:triage", "state:confirmed", "state:resolved"].map(option),
        );

        let active: Vec<bool> = bar.pills.iter().map(|pill| pill.active).collect();
        assert_eq!(active, [true, true, false]);
    }

    #[test]
    fn a_pill_is_active_whatever_the_case_or_quoting_of_the_query() {
        let bar = FilterBar::new(
            "Status",
            "/",
            DEFAULT,
            "kind:crash STATE:\"Resolved\"",
            ["state:triage", "state:resolved"].map(option),
        );

        let active: Vec<bool> = bar.pills.iter().map(|pill| pill.active).collect();
        assert_eq!(active, [false, true]);
    }

    #[test]
    fn a_plain_click_on_an_inactive_pill_replaces_the_query() {
        assert_eq!(
            plain_click_query("kind:crash state:triage", DEFAULT, "state:resolved"),
            "state:resolved"
        );
    }

    #[test]
    fn a_plain_click_on_the_only_active_pill_restores_the_default_filters_and_keeps_the_rest() {
        assert_eq!(
            plain_click_query("state:resolved", DEFAULT, "state:resolved"),
            DEFAULT
        );
        assert_eq!(
            plain_click_query(
                "version:\"Ladybird Nightly\" state:triage",
                DEFAULT,
                "state:triage",
            ),
            "version:\"Ladybird Nightly\" state:triage state:confirmed"
        );
    }

    #[test]
    fn a_plain_click_on_one_of_several_active_pills_keeps_only_that_one() {
        assert_eq!(
            plain_click_query(DEFAULT, DEFAULT, "state:confirmed"),
            "state:confirmed"
        );
        assert_eq!(
            plain_click_query(
                "state:triage kind:crash state:rejected",
                DEFAULT,
                "state:rejected"
            ),
            "kind:crash state:rejected"
        );
    }

    #[test]
    fn the_link_of_a_pill_is_its_plain_click() {
        let bar = FilterBar::new(
            "Status",
            "/issues",
            DEFAULT,
            "state:resolved",
            ["state:resolved", "state:triage"].map(option),
        );

        assert_eq!(
            bar.pills[0].href,
            "/issues?q=state%3Atriage+state%3Aconfirmed"
        );
        assert_eq!(bar.pills[1].href, "/issues?q=state%3Atriage");
    }

    #[test]
    fn quoted_values_stay_whole() {
        assert_eq!(
            tokens(r#"a version:"Ladybird \"Nightly\"" b"#),
            ["a", r#"version:"Ladybird \"Nightly\"""#, "b"]
        );
    }
}
