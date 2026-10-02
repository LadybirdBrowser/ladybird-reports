const log = document.querySelector("[data-audit-log]");

async function loadMore(button) {
    const status = log.querySelector("[data-audit-status]");

    button.disabled = true;
    button.textContent = "Loading…";
    status.textContent = "Loading more audit events…";

    try {
        const response = await fetch(button.dataset.nextUrl, {
            headers: { Accept: "text/html" },
        });
        if (!response.ok) {
            throw new Error(`Audit log page returned ${response.status}`);
        }

        const page = new DOMParser().parseFromString(await response.text(), "text/html");
        const nextRows = page.querySelector("[data-audit-rows]");
        const nextButton = page.querySelector("[data-audit-show-more]");
        const currentRows = log.querySelector("[data-audit-rows]");

        for (const row of Array.from(nextRows?.children ?? [])) {
            currentRows.append(row);
        }

        if (nextButton) {
            // Keep the same button so keyboard focus stays where it was.
            button.dataset.nextUrl = nextButton.dataset.nextUrl;
            button.disabled = false;
            button.textContent = "Show more…";
        } else {
            button.closest(".report-list-more").remove();
        }

        status.textContent = "More audit events loaded.";
    } catch {
        button.disabled = false;
        button.textContent = "Show more…";
        status.textContent = "More audit events could not be loaded.";
    }
}

log?.addEventListener("click", (event) => {
    const button = event.target.closest("[data-audit-show-more]");
    if (button) {
        loadMore(button);
    }
});
