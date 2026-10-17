// The quick filters above a list (_filter_bar.html). Each pill stands for one search qualifier and toggles it in the
// search box of the list, which then updates the list as if it had been typed there. What a click does:
//
// - a plain click on a pill that is not active replaces the query with its qualifier;
// - a plain click on an active pill, when others of its kind are active too, keeps only that one;
// - a plain click on the only active pill of its kind removes it and puts the default ones back, leaving the rest of
//   the query alone;
// - with Cmd or Ctrl, a pill that is not active adds its qualifier to the query, and an active one removes it.
//
// The links of the pills lead to what a plain click does, for when this script is not running. The same rules are
// written in src/web/admin/filter_bar.rs.

const input = document.querySelector("[data-list-search-input]");

// The words of a query, with quoted values kept whole and as written.
function tokens(query) {
    return query.match(/(?:[^\s"]|"(?:\\.|[^"\\])*"?)+/g) ?? [];
}

// The key and value of a qualifier, lowercase and without the quotes around the value.
function qualifier(token) {
    const separator = token.indexOf(":");
    if (separator < 0) {
        return null;
    }

    let value = token.slice(separator + 1);
    if (value.length >= 2 && value.startsWith('"') && value.endsWith('"')) {
        value = value.slice(1, -1);
    }
    return { key: token.slice(0, separator).toLowerCase(), value: value.toLowerCase() };
}

function matches(token, wanted) {
    const found = qualifier(token);
    return found !== null && found.key === wanted.key && found.value === wanted.value;
}

function contains(query, expression) {
    const wanted = qualifier(expression);
    return wanted !== null && tokens(query).some((token) => matches(token, wanted));
}

function queryAfterClick(query, defaultQuery, expression, additive) {
    const wanted = qualifier(expression);
    const present = contains(query, expression);

    if (additive) {
        return (present
            ? tokens(query).filter((token) => !matches(token, wanted))
            : [...tokens(query), expression]
        ).join(" ");
    }

    if (!present) {
        return expression;
    }

    const ofOtherKinds = tokens(query).filter((token) => qualifier(token)?.key !== wanted.key);
    const othersOfKind = tokens(query).some(
        (token) => qualifier(token)?.key === wanted.key && !matches(token, wanted),
    );
    if (othersOfKind) {
        return [...ofOtherKinds, expression].join(" ");
    }

    const defaults = tokens(defaultQuery).filter((token) => qualifier(token)?.key === wanted.key);
    return [...ofOtherKinds, ...defaults].join(" ");
}

function refresh(bar) {
    for (const pill of bar.querySelectorAll("[data-filter-expression]")) {
        const active = contains(input.value, pill.dataset.filterExpression);
        pill.classList.toggle("is-active", active);
        if (active) {
            pill.setAttribute("aria-current", "true");
        } else {
            pill.removeAttribute("aria-current");
        }
    }
}

function connect(bar) {
    bar.addEventListener("click", (event) => {
        const pill = event.target.closest("[data-filter-expression]");
        if (!pill || event.button !== 0 || event.shiftKey || event.altKey) {
            return;
        }

        event.preventDefault();
        input.value = queryAfterClick(
            input.value,
            bar.dataset.filterDefault,
            pill.dataset.filterExpression,
            event.metaKey || event.ctrlKey,
        );
        refresh(bar);
        input.form.requestSubmit();
    });
    input.addEventListener("input", () => refresh(bar));
}

if (input) {
    for (const bar of document.querySelectorAll("[data-filter-bar]")) {
        connect(bar);
    }
}
