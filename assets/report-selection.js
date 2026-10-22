// Selecting reports in the list, and the bar of actions that apply to every selected report at once. The selection is
// whatever is checked in the list, so "Show more…" keeps it and a new search, which replaces the list, clears it.

const reportList = document.querySelector("[data-list-results]");
const bulkActions = document.querySelector("[data-bulk-actions]");
const BULK_REPORT_LIMIT = 200;

function reportCheckboxes() {
    return Array.from(reportList.querySelectorAll("[data-report-select]"));
}

function selectedReports() {
    const checked = reportCheckboxes().filter((checkbox) => checkbox.checked);
    return Array.from(new Set(checked.map((checkbox) => checkbox.value)));
}

// The checkboxes of a group's reports, which follow its header row (see report-groups.js).
function groupMembers(groupCheckbox) {
    const header = groupCheckbox.closest("tr");
    const checkboxes = [];
    for (
        let row = header.nextElementSibling;
        row?.classList.contains("report-group-member") && row.dataset.signature === header.dataset.group;
        row = row.nextElementSibling
    ) {
        checkboxes.push(...row.querySelectorAll("[data-report-select]"));
    }
    return checkboxes;
}

// A checkbox for several reports shows whether all, some, or none of them are selected.
function reflect(checkbox, members) {
    const checked = members.filter((member) => member.checked).length;
    checkbox.checked = members.length > 0 && checked === members.length;
    checkbox.indeterminate = checked > 0 && checked < members.length;
}

function describeCount(count) {
    return count === 1 ? "1 report" : `${count} reports`;
}

function update() {
    for (const groupCheckbox of reportList.querySelectorAll("[data-report-select-group]")) {
        reflect(groupCheckbox, groupMembers(groupCheckbox));
    }
    const all = reportList.querySelector("[data-report-select-all]");
    if (all) {
        reflect(all, reportCheckboxes());
    }

    const reports = selectedReports();
    bulkActions.hidden = reports.length === 0;
    if (reports.length === 0) {
        bulkActions.querySelector("[data-bulk-error]").hidden = true;
    }
    bulkActions.querySelector("[data-bulk-count]").textContent = describeCount(reports.length);
    for (const text of document.querySelectorAll("[data-bulk-count-text]")) {
        text.textContent = describeCount(reports.length);
    }

    const overLimit = reports.length > BULK_REPORT_LIMIT;
    bulkActions.querySelector("[data-bulk-limit]").hidden = !overLimit;
    for (const button of bulkActions.querySelectorAll("[data-bulk-action]")) {
        button.disabled = overLimit;
    }
}

if (reportList && bulkActions) {
    reportList.addEventListener("change", (event) => {
        const checkbox = event.target;
        if (checkbox.matches("[data-report-select-all]")) {
            for (const member of reportCheckboxes()) {
                member.checked = checkbox.checked;
            }
        } else if (checkbox.matches("[data-report-select-group]")) {
            for (const member of groupMembers(checkbox)) {
                member.checked = checkbox.checked;
            }
        }
        update();
    });

    bulkActions.querySelector("[data-bulk-clear]").addEventListener("click", () => {
        for (const checkbox of reportCheckboxes()) {
            checkbox.checked = false;
        }
        update();
    });

    // Every form of the bar, and of the dialog it opens, sends the selection, and the list to return to. An action
    // applies to all of the reports or to none, so a refusal is shown in the bar, with the selection kept to adjust.
    const error = bulkActions.querySelector("[data-bulk-error]");
    for (const form of document.querySelectorAll("[data-bulk-form]")) {
        form.addEventListener("submit", async (event) => {
            event.preventDefault();
            form.querySelector("[data-bulk-reports]").value = selectedReports().join(" ");
            form.querySelector("[data-bulk-return-to]").value = window.location.pathname + window.location.search;
            error.hidden = true;

            let message = "The reports could not be changed.";
            try {
                // Not form.action, which the form's own "action" field shadows.
                const response = await fetch(form.getAttribute("action"), {
                    method: "POST",
                    body: new URLSearchParams(new FormData(form)),
                });
                if (response.ok) {
                    window.location.assign(response.url);
                    return;
                }
                message = (await response.json()).error ?? message;
            } catch {
                // Keep the general message.
            }

            form.closest("dialog")?.close();
            form.closest("[popover]")?.hidePopover();
            error.textContent = message;
            error.hidden = false;
        });
    }

    for (const trigger of bulkActions.querySelectorAll("[data-bulk-dialog-open]")) {
        trigger.addEventListener("click", () => {
            document.getElementById(trigger.dataset.bulkDialogOpen).showModal();
        });
    }
    for (const dialog of document.querySelectorAll("dialog.modal-overlay")) {
        dialog.addEventListener("click", (event) => {
            if (event.target === dialog || event.target.closest("[data-dialog-close]")) {
                dialog.close();
            }
        });
    }

    // A search replaces the list and "Show more…" adds to it; grouping moves rows into place.
    new MutationObserver(update).observe(reportList, { childList: true, subtree: true });
    update();
}
