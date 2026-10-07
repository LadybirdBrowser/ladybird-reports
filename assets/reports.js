// A dialog opens from a `data-dialog-open` button and closes from any
// `data-dialog-close` element inside it or a click on its backdrop.
for (const trigger of document.querySelectorAll("[data-dialog-open]")) {
    trigger.addEventListener("click", () => {
        document.getElementById(trigger.dataset.dialogOpen).showModal();
    });
}
for (const dialog of document.querySelectorAll("dialog.modal-overlay")) {
    dialog.addEventListener("click", (event) => {
        if (event.target === dialog || event.target.closest("[data-dialog-close]")) {
            dialog.close();
        }
    });
}

function initializeIssueDialog() {
    const dialog = document.getElementById("issue-dialog");
    if (!dialog) {
        return;
    }

    const modeButtons = Array.from(dialog.querySelectorAll("[data-issue-mode]"));
    const panels = Array.from(dialog.querySelectorAll("[data-issue-panel]"));

    const selectMode = (mode) => {
        for (const button of modeButtons) {
            button.setAttribute("aria-pressed", String(button.dataset.issueMode === mode));
        }
        for (const panel of panels) {
            panel.hidden = panel.dataset.issuePanel !== mode;
        }

        const panel = panels.find((candidate) => candidate.dataset.issuePanel === mode);
        panel?.querySelector("input:not([type=hidden]), textarea")?.focus();
    };

    document.querySelector('[data-dialog-open="issue-dialog"]')
        ?.addEventListener("click", () => selectMode("existing"));
    for (const button of modeButtons) {
        button.addEventListener("click", () => selectMode(button.dataset.issueMode));
    }
}

initializeIssueDialog();

// Addresses come from the reporter, so they are only opened after a warning.
function initializeUrlDialog() {
    const dialog = document.querySelector("[data-url-dialog]");
    if (!dialog) {
        return;
    }

    const value = dialog.querySelector("[data-url-dialog-value]");
    const openLink = dialog.querySelector("[data-url-dialog-open]");

    for (const trigger of document.querySelectorAll("[data-untrusted-url]")) {
        trigger.addEventListener("click", () => {
            const address = trigger.dataset.untrustedUrl.trim();
            let protocol;
            try {
                protocol = new URL(address).protocol;
            } catch {
                return;
            }
            if (protocol !== "http:" && protocol !== "https:") {
                return;
            }

            value.value = address;
            openLink.href = address;
            dialog.showModal();
        });
    }
}

initializeUrlDialog();

async function writeToClipboard(text) {
    if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(text);
        return;
    }

    // Without the asynchronous API, which needs a secure context, select the
    // text of a temporary field and use the older command.
    const field = document.createElement("textarea");
    field.value = text;
    field.setAttribute("readonly", "");
    field.style.position = "fixed";
    field.style.opacity = "0";
    document.body.append(field);
    field.select();
    try {
        if (!document.execCommand("copy")) {
            throw new Error("The browser refused to copy.");
        }
    } finally {
        field.remove();
    }
}

function initializeCopyButtons() {
    const status = document.createElement("div");
    status.className = "visually-hidden";
    status.setAttribute("role", "status");
    document.body.append(status);

    for (const button of document.querySelectorAll("[data-copy]")) {
        const label = button.getAttribute("aria-label");
        const title = button.title;
        let timer;

        button.addEventListener("click", async () => {
            const source = button.closest("[data-copy-scope]")?.querySelector("[data-copy-source]");
            let state = "copied";
            try {
                await writeToClipboard(source?.value ?? source?.textContent ?? "");
            } catch {
                state = "failed";
            }

            const message = state === "copied" ? "Copied" : "Copy failed";
            button.dataset.copyState = state;
            button.title = message;
            status.textContent = `${message}: ${label}`;

            clearTimeout(timer);
            timer = setTimeout(() => {
                delete button.dataset.copyState;
                button.title = title;
                status.textContent = "";
            }, 1800);
        });
    }
}

initializeCopyButtons();
