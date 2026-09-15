const SEARCH_COMPLETION_DELAY_MS = 120;

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
            selectMode("create");
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

class ReportListController {
    constructor(form) {
        this.form = form;
        this.input = form.querySelector("[data-report-search-input]");
        this.list = document.querySelector("[data-report-list]");
        this.status = document.querySelector("[data-report-search-status]");
        this.completions = form.querySelector("[data-report-search-completions]");
        this.completionTimer = null;
        this.searchRequest = null;
        this.completionRequest = null;
        this.options = [];
        this.activeOption = -1;
        this.tokenRange = null;
    }

    connect() {
        this.form.addEventListener("submit", (event) => {
            event.preventDefault();
            this.updateResults();
        });
        this.input.addEventListener("input", () => {
            this.closeCompletions();
            this.queueCompletions();
        });
        this.form.addEventListener("focusout", () => {
            setTimeout(() => {
                if (!this.form.contains(document.activeElement)) {
                    this.updateResults();
                }
            });
        });
        this.input.addEventListener("focus", () => this.queueCompletions());
        this.input.addEventListener("click", () => this.queueCompletions());
        this.input.addEventListener("keydown", (event) => this.handleKeydown(event));
        this.list.addEventListener("click", (event) => {
            const button = event.target.closest("[data-report-show-more]");
            if (button) {
                this.loadMore(button);
            }
        });
        this.completions.addEventListener("click", (event) => {
            const button = event.target.closest("[data-search-completion]");
            if (button) {
                this.applyCompletion(Number(button.dataset.searchCompletion));
            }
        });
        document.addEventListener("click", (event) => {
            if (!this.form.contains(event.target)) {
                this.closeCompletions();
            }
        });
        window.addEventListener("resize", () => this.positionCompletions());
        window.addEventListener("scroll", () => this.positionCompletions(), true);
    }

    queueCompletions() {
        clearTimeout(this.completionTimer);
        this.completionTimer = setTimeout(
            () => this.loadCompletions(),
            SEARCH_COMPLETION_DELAY_MS,
        );
    }

    async updateResults() {
        this.closeCompletions();
        this.searchRequest?.abort();
        this.searchRequest = new AbortController();

        const pageUrl = new URL(window.location.href);
        pageUrl.searchParams.set("q", this.input.value.trim());
        pageUrl.searchParams.delete("before");
        pageUrl.searchParams.delete("before_id");
        const requestUrl = new URL("/api/report-list", window.location.origin);
        requestUrl.search = pageUrl.search;

        this.list.setAttribute("aria-busy", "true");
        this.status.textContent = "Updating reports…";
        try {
            const response = await fetch(requestUrl, {
                headers: { Accept: "text/html" },
                signal: this.searchRequest.signal,
            });
            if (!response.ok) {
                throw new Error(`Report search returned ${response.status}`);
            }

            const content = this.parseList(await response.text());
            this.list.replaceChildren(content);
            history.replaceState({}, "", pageUrl);
            this.status.textContent = "Reports updated.";
        } catch (error) {
            if (error.name !== "AbortError") {
                this.status.textContent = "Reports could not be updated.";
            }
        } finally {
            this.list.removeAttribute("aria-busy");
        }
    }

    async loadMore(button) {
        button.disabled = true;
        button.textContent = "Loading…";
        this.status.textContent = "Loading more reports…";

        try {
            const response = await fetch(button.dataset.nextUrl, {
                headers: { Accept: "text/html" },
            });
            if (!response.ok) {
                throw new Error(`Report page returned ${response.status}`);
            }

            const nextContent = this.parseList(await response.text());
            const currentRows = this.list.querySelector("[data-report-rows]");
            const nextRows = nextContent.querySelector("[data-report-rows]");
            for (const row of Array.from(nextRows?.children ?? [])) {
                currentRows.append(row);
            }

            this.list.querySelector(".report-list-more")?.remove();
            const nextMore = nextContent.querySelector(".report-list-more");
            if (nextMore) {
                this.list.querySelector("[data-report-list-content]").append(nextMore);
            }
            this.status.textContent = "More reports loaded.";
        } catch {
            button.disabled = false;
            button.textContent = "Show more…";
            this.status.textContent = "More reports could not be loaded.";
        }
    }

    parseList(html) {
        const document = new DOMParser().parseFromString(html, "text/html");
        const content = document.querySelector("[data-report-list-content]");
        if (!content) {
            throw new Error("Report response did not contain a list");
        }
        return content;
    }

    async loadCompletions() {
        const range = activeTokenRange(this.input.value, this.input.selectionStart);
        this.tokenRange = range;
        this.completionRequest?.abort();
        this.completionRequest = new AbortController();

        const url = new URL("/api/report-search-completions", window.location.origin);
        url.searchParams.set("token", this.input.value.slice(range.start, range.end));
        try {
            const response = await fetch(url, {
                headers: { Accept: "application/json" },
                signal: this.completionRequest.signal,
            });
            if (!response.ok) {
                throw new Error(`Completion request returned ${response.status}`);
            }

            const payload = await response.json();
            this.options = payload.results;
            this.activeOption = this.options.length > 0 ? 0 : -1;
            this.renderCompletions();
        } catch (error) {
            if (error.name !== "AbortError") {
                this.closeCompletions();
            }
        }
    }

    renderCompletions() {
        this.completions.replaceChildren();
        if (this.options.length === 0) {
            this.closeCompletions();
            return;
        }

        for (const [index, option] of this.options.entries()) {
            const button = createReportElement("button", "search-completion");
            button.type = "button";
            button.role = "option";
            button.dataset.searchCompletion = String(index);
            button.setAttribute("aria-selected", String(index === this.activeOption));
            button.append(
                createReportElement("strong", "", option.label),
                createReportElement("span", "", option.description),
            );
            this.completions.append(button);
        }

        if (!this.completions.matches(":popover-open")) {
            this.completions.showPopover();
        }
        this.input.setAttribute("aria-expanded", "true");
        this.positionCompletions();
    }

    positionCompletions() {
        if (!this.completions.matches(":popover-open")) {
            return;
        }
        const bounds = this.input.getBoundingClientRect();
        this.completions.style.left = `${bounds.left}px`;
        this.completions.style.top = `${bounds.bottom + 6}px`;
        this.completions.style.width = `${bounds.width}px`;
    }

    handleKeydown(event) {
        if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            if (this.options.length === 0) {
                return;
            }
            event.preventDefault();
            const direction = event.key === "ArrowDown" ? 1 : -1;
            this.activeOption = Math.max(
                0,
                Math.min(this.activeOption + direction, this.options.length - 1),
            );
            this.renderCompletions();
        } else if ((event.key === "Enter" || event.key === "Tab") && this.activeOption >= 0) {
            event.preventDefault();
            this.applyCompletion(this.activeOption);
        } else if (event.key === "Escape") {
            this.closeCompletions();
        } else if (event.key === "Enter") {
            event.preventDefault();
            this.updateResults();
        }
    }

    applyCompletion(index) {
        const option = this.options[index];
        if (!option || !this.tokenRange) {
            return;
        }

        const before = this.input.value.slice(0, this.tokenRange.start);
        const after = this.input.value.slice(this.tokenRange.end);
        this.input.value = `${before}${option.replacement}${after}`;
        const caret = before.length + option.replacement.length;
        this.input.setSelectionRange(caret, caret);
        this.closeCompletions();
        this.input.focus();
        if (option.replacement.endsWith(":")) {
            this.queueCompletions();
        } else {
            this.updateResults();
        }
    }

    closeCompletions() {
        clearTimeout(this.completionTimer);
        this.completionRequest?.abort();
        this.completionRequest = null;
        this.tokenRange = null;
        this.options = [];
        this.activeOption = -1;
        if (this.completions.matches(":popover-open")) {
            this.completions.hidePopover();
        }
        this.input.setAttribute("aria-expanded", "false");
    }
}

function activeTokenRange(value, caret) {
    let start = 0;
    let quoted = false;
    for (let index = 0; index < caret; index += 1) {
        if (value[index] === '"' && value[index - 1] !== "\\") {
            quoted = !quoted;
        } else if (/\s/.test(value[index]) && !quoted) {
            start = index + 1;
        }
    }

    let end = value.length;
    for (let index = caret; index < value.length; index += 1) {
        if (value[index] === '"' && value[index - 1] !== "\\") {
            quoted = !quoted;
        } else if (/\s/.test(value[index]) && !quoted) {
            end = index;
            break;
        }
    }
    return { start, end };
}

function createReportElement(tagName, className, text) {
    const element = document.createElement(tagName);
    if (className) {
        element.className = className;
    }
    if (text !== undefined) {
        element.textContent = text;
    }
    return element;
}

function initializeReportLists() {
    for (const form of document.querySelectorAll("[data-report-search]")) {
        new ReportListController(form).connect();
    }
}

initializeIssueDialogs();
initializeReportLists();
