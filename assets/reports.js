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
