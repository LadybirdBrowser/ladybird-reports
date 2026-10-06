function initializeIssueDialogs() {
    for (const dialog of document.querySelectorAll("[data-issue-dialog]")) {
        const openButton = document.querySelector("[data-open-issue-dialog]");
        const closeButtons = dialog.querySelectorAll("[data-close-issue-dialog]");
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

        openButton?.addEventListener("click", () => {
            dialog.showModal();
            selectMode("existing");
        });
        for (const button of closeButtons) {
            button.addEventListener("click", () => dialog.close());
        }
        for (const button of modeButtons) {
            button.addEventListener("click", () => selectMode(button.dataset.issueMode));
        }
        dialog.addEventListener("click", (event) => {
            if (event.target === dialog) {
                dialog.close();
            }
        });
    }
}

initializeIssueDialogs();

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
                await writeToClipboard(source?.textContent ?? "");
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
