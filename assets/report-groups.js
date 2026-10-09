// Groups the rows of the report list that have the same stack signature. Only
// the display is grouped. Rows arrive newest first, so the first row of a
// signature is its most recent report and the group takes its place; rows that
// "Show more…" loads later join the group that is already there.

const results = document.querySelector("[data-list-results]");

function element(tag, className, text) {
    const created = document.createElement(tag);
    created.className = className;
    if (text) {
        created.textContent = text;
    }
    return created;
}

function members(header) {
    const rows = [];
    for (
        let row = header.nextElementSibling;
        row?.classList.contains("report-group-member") && row.dataset.signature === header.dataset.group;
        row = row.nextElementSibling
    ) {
        rows.push(row);
    }
    return rows;
}

// The header repeats the most recent report, as a button that opens the group.
function createHeader(newest) {
    const header = newest.cloneNode(true);
    header.classList.add("report-group-row");
    header.dataset.group = newest.dataset.signature;
    delete header.dataset.signature;

    const link = header.querySelector(".report-title-link");
    const toggle = element("button", "report-group-toggle row-link");
    toggle.type = "button";
    toggle.setAttribute("aria-expanded", "false");
    toggle.append(
        element("span", "report-group-chevron"),
        element("span", "report-group-title", link.textContent.trim()),
        element("span", "badge badge-neutral report-group-count"),
        // The pill only shows the number; screen readers get the unit as well.
        element("span", "visually-hidden", " reports"),
    );
    toggle.firstElementChild.setAttribute("aria-hidden", "true");
    link.replaceWith(toggle);
    header.querySelector(".report-table-metadata").hidden = true;

    return header;
}

function setExpanded(header, expanded) {
    header.querySelector(".report-group-toggle").setAttribute("aria-expanded", String(expanded));
    for (const row of members(header)) {
        row.hidden = !expanded;
    }
}

function describe(header) {
    const rows = members(header);
    const states = new Map();
    for (const row of rows) {
        const state = row.querySelector(".badge")?.textContent.trim();
        states.set(state, (states.get(state) ?? 0) + 1);
    }

    header.querySelector(".report-group-count").textContent = rows.length;
    header.querySelector(".badge:not(.report-group-count)").title =
        Array.from(states, ([state, count]) => `${count} ${state}`).join(" · ");
}

function join(header, row) {
    let last = header;
    for (const member of members(header)) {
        last = member;
    }

    row.classList.add("report-group-member");
    row.hidden = header.querySelector(".report-group-toggle").getAttribute("aria-expanded") !== "true";
    last.after(row);
    describe(header);
}

function organize() {
    const rows = results.querySelector("[data-rows]");
    if (!rows) {
        return;
    }

    const headers = new Map();
    const singles = new Map();
    for (const row of Array.from(rows.children)) {
        const signature = row.dataset.signature;
        if (row.classList.contains("report-group-row")) {
            headers.set(row.dataset.group, row);
        } else if (signature && !row.classList.contains("report-group-member")) {
            if (headers.has(signature)) {
                join(headers.get(signature), row);
            } else if (singles.has(signature)) {
                const first = singles.get(signature);
                const header = createHeader(first);
                first.before(header);
                headers.set(signature, header);
                join(header, first);
                join(header, row);
                singles.delete(signature);
            } else {
                singles.set(signature, row);
            }
        }
    }

    // A list that is one group is not worth a click.
    const header = headers.values().next().value;
    if (headers.size === 1 && !header.dataset.touched && rows.children.length === members(header).length + 1) {
        setExpanded(header, true);
    }
}

results?.addEventListener("click", (event) => {
    const toggle = event.target.closest(".report-group-toggle");
    if (toggle) {
        const header = toggle.closest("tr");
        header.dataset.touched = "true";
        setExpanded(header, toggle.getAttribute("aria-expanded") !== "true");
    }
});

if (results) {
    // The list is replaced by a search and grows by "Show more…". Reorganizing
    // moves rows around, which the observer would see too, so it is paused.
    const observer = new MutationObserver(() => {
        observer.disconnect();
        organize();
        observer.observe(results, { childList: true, subtree: true });
    });
    observer.observe(results, { childList: true, subtree: true });
    organize();
}
