const ENTITY_SEARCH_DELAY_MS = 180;
const FILTER_SUBMIT_DELAY_MS = 650;

class EntitySelector {
    constructor(root) {
        this.root = root;
        this.searchUrl = root.dataset.searchUrl;
        this.entitySingular = root.dataset.entitySingular ?? "item";
        this.entityPlural = root.dataset.entityPlural ?? "items";
        this.maximumSelections = Number(root.dataset.maximumSelections ?? 1);

        this.valueInput = root.querySelector("[data-entity-value]");
        this.searchInput = root.querySelector("[data-entity-search]");
        this.selection = root.querySelector("[data-entity-selection]");
        this.selectionCount = root.querySelector("[data-entity-selection-count]");
        this.chips = root.querySelector("[data-entity-chips]");
        this.popover = root.querySelector("[data-entity-popover]");
        this.results = root.querySelector("[data-entity-results]");
        this.resultCount = root.querySelector("[data-entity-result-count]");
        this.empty = root.querySelector("[data-entity-empty]");
        this.status = root.querySelector("[data-entity-status]");

        this.selected = new Map();
        this.options = [];
        this.activeIndex = -1;
        this.hasLoaded = false;
        this.searchTimer = null;
        this.request = null;
    }

    connect() {
        this.searchInput.addEventListener("focus", () => {
            this.open();

            if (!this.hasLoaded) {
                this.loadResults();
            }
        });
        this.searchInput.addEventListener("input", () => this.queueSearch());
        this.searchInput.addEventListener("keydown", (event) => this.handleKeydown(event));

        this.results.addEventListener("mousemove", (event) => {
            const option = event.target.closest("[data-entity-option]");
            if (!option) {
                return;
            }

            this.setActiveIndex(Number(option.dataset.entityOption));
        });

        this.results.addEventListener("click", (event) => {
            const option = event.target.closest("[data-entity-option]");
            if (!option) {
                return;
            }

            this.selectOption(this.options[Number(option.dataset.entityOption)]);
        });

        this.chips.addEventListener("click", (event) => {
            const removeButton = event.target.closest("[data-entity-remove]");
            if (removeButton) {
                this.removeSelection(removeButton.dataset.entityRemove);
            }
        });

        this.popover.addEventListener("beforetoggle", (event) => {
            const isOpening = event.newState === "open";
            this.searchInput.setAttribute("aria-expanded", String(isOpening));
            if (isOpening) {
                this.positionPopover();
            } else {
                this.searchInput.removeAttribute("aria-activedescendant");
                this.activeIndex = -1;
            }
        });

        const reposition = () => {
            if (this.popover.matches(":popover-open")) {
                this.positionPopover();
            }
        };
        window.addEventListener("resize", reposition);
        window.addEventListener("scroll", reposition, true);

        document.addEventListener("click", (event) => {
            if (!this.root.contains(event.target)) {
                this.close();
            }
        });

        this.renderSelection();
    }

    focus() {
        this.searchInput.focus();
    }

    open() {
        if (!this.popover.matches(":popover-open")) {
            this.popover.showPopover();
        }
    }

    close() {
        if (this.popover.matches(":popover-open")) {
            this.popover.hidePopover();
        }
        this.searchInput.removeAttribute("aria-activedescendant");
        this.activeIndex = -1;
    }

    positionPopover() {
        const bounds = this.searchInput.getBoundingClientRect();
        const viewportMargin = 8;
        const preferredWidth = Math.max(bounds.width, 544);
        const width = Math.min(preferredWidth, window.innerWidth - viewportMargin * 2);
        const left = Math.max(
            viewportMargin,
            Math.min(bounds.right - width, window.innerWidth - width - viewportMargin),
        );
        const availableBelow = window.innerHeight - bounds.bottom - viewportMargin;
        const openAbove = availableBelow < 280 && bounds.top > availableBelow;

        this.popover.style.width = `${width}px`;
        this.popover.style.left = `${left}px`;
        this.popover.style.top = openAbove ? "auto" : `${bounds.bottom + 6}px`;
        this.popover.style.bottom = openAbove
            ? `${window.innerHeight - bounds.top + 6}px`
            : "auto";
    }

    queueSearch() {
        window.clearTimeout(this.searchTimer);
        this.searchTimer = window.setTimeout(
            () => this.loadResults(),
            ENTITY_SEARCH_DELAY_MS,
        );
    }

    async loadResults() {
        const query = this.searchInput.value.trim();

        this.request?.abort();
        this.request = new AbortController();
        this.open();
        this.renderLoading();

        const url = new URL(this.searchUrl, window.location.origin);
        url.searchParams.set("query", query);

        try {
            const response = await fetch(url, {
                headers: { Accept: "application/json" },
                signal: this.request.signal,
            });

            if (!response.ok) {
                throw new Error(`Search returned ${response.status}`);
            }

            const payload = await response.json();
            this.hasLoaded = true;
            this.options = payload.results
                .map(normalizeEntityOption)
                .filter((option) => !this.selected.has(option.value));
            this.activeIndex = this.options.length > 0 ? 0 : -1;
            this.renderResults(query);
        } catch (error) {
            if (error.name !== "AbortError") {
                this.renderError();
            }
        }
    }

    renderLoading() {
        this.results.replaceChildren();
        this.empty.hidden = true;
        this.resultCount.textContent = "Searching";

        for (let index = 0; index < 3; index += 1) {
            const skeleton = document.createElement("div");
            skeleton.className = "entity-selector-skeleton";
            skeleton.setAttribute("aria-hidden", "true");

            const icon = document.createElement("span");
            icon.className = "entity-selector-skeleton-icon";

            const copy = document.createElement("span");
            copy.className = "entity-selector-skeleton-copy";
            copy.append(
                createElement("span", "entity-selector-skeleton-line is-wide"),
                createElement("span", "entity-selector-skeleton-line"),
            );

            skeleton.append(icon, copy);
            this.results.append(skeleton);
        }
    }

    renderResults(query) {
        this.results.replaceChildren();
        this.empty.hidden = this.options.length > 0;
        this.resultCount.textContent = formatCount(
            this.options.length,
            this.entitySingular,
            this.entityPlural,
        );

        let previousGroup = null;
        for (const [index, option] of this.options.entries()) {
            if (option.group && option.group !== previousGroup) {
                this.results.append(
                    createElement("div", "entity-selector-group", option.group),
                );
                previousGroup = option.group;
            }
            this.results.append(this.createOption(option, index, query));
        }

        this.updateActiveOption();
        this.status.textContent = this.options.length === 0
            ? `No ${this.entityPlural} found.`
            : `${formatCount(this.options.length, this.entitySingular, this.entityPlural)} available.`;
    }

    renderError() {
        this.options = [];
        this.activeIndex = -1;
        this.results.replaceChildren();
        this.empty.hidden = false;
        this.empty.querySelector("strong").textContent = "Could not load results";
        this.empty.querySelector("span:last-child").textContent =
            "Check your connection and try searching again.";
        this.resultCount.textContent = "Unavailable";
        this.status.textContent = `Could not load ${this.entityPlural}.`;
    }

    createOption(option, index, query) {
        const button = document.createElement("button");
        button.id = `${this.results.id}-option-${index}`;
        button.className = "entity-selector-option";
        button.type = "button";
        button.role = "option";
        button.dataset.entityOption = String(index);
        button.setAttribute("aria-selected", "false");

        const icon = createElement("span", "entity-selector-option-icon");
        icon.setAttribute("aria-hidden", "true");

        const content = createElement("span", "entity-selector-option-content");
        const heading = createElement("span", "entity-selector-option-heading");
        heading.append(createHighlightedText(option.label, query));

        const badge = createElement(
            "span",
            `entity-selector-badge is-${option.badgeTone}`,
            option.badge,
        );
        heading.append(badge);

        const description = createElement("span", "entity-selector-option-description");
        description.append(createHighlightedText(option.description, query));

        const metadata = createElement("span", "entity-selector-option-metadata");
        const identifier = createElement("code", "entity-selector-option-identifier");
        identifier.append(createHighlightedText(option.identifier, query));
        metadata.append(identifier, createElement("span", "", option.footnote));

        content.append(heading, description, metadata);
        button.append(icon, content);

        return button;
    }

    handleKeydown(event) {
        if (event.key === "ArrowDown") {
            event.preventDefault();
            this.moveActiveOption(1);
        } else if (event.key === "ArrowUp") {
            event.preventDefault();
            this.moveActiveOption(-1);
        } else if (event.key === "Enter" && this.activeIndex >= 0) {
            event.preventDefault();
            this.selectOption(this.options[this.activeIndex]);
        } else if (event.key === "Escape") {
            event.preventDefault();
            this.close();
        } else if (
            event.key === "Backspace"
            && this.searchInput.value === ""
            && this.selected.size > 0
        ) {
            const lastValue = Array.from(this.selected.keys()).at(-1);
            this.removeSelection(lastValue);
        }
    }

    moveActiveOption(direction) {
        if (!this.popover.matches(":popover-open")) {
            this.open();
            return;
        }

        if (this.options.length === 0) {
            return;
        }

        const nextIndex = this.activeIndex + direction;
        this.activeIndex = Math.max(0, Math.min(nextIndex, this.options.length - 1));
        this.updateActiveOption();
    }

    setActiveIndex(index) {
        if (index === this.activeIndex) {
            return;
        }

        this.activeIndex = index;
        this.updateActiveOption();
    }

    updateActiveOption() {
        const optionElements = this.results.querySelectorAll("[data-entity-option]");

        for (const [index, optionElement] of optionElements.entries()) {
            const isActive = index === this.activeIndex;
            optionElement.classList.toggle("is-active", isActive);
            optionElement.setAttribute("aria-selected", String(isActive));

            if (isActive) {
                this.searchInput.setAttribute("aria-activedescendant", optionElement.id);
                optionElement.scrollIntoView({ block: "nearest" });
            }
        }
    }

    selectOption(option) {
        if (!option || this.selected.size >= this.maximumSelections) {
            return;
        }

        this.selected.set(option.value, option);
        this.searchInput.value = "";
        this.renderSelection();
        this.loadResults();
        this.searchInput.focus();
        this.status.textContent = `${option.label} selected.`;
    }

    removeSelection(value) {
        const option = this.selected.get(value);
        if (!option) {
            return;
        }

        this.selected.delete(value);
        this.renderSelection();
        this.loadResults();
        this.searchInput.focus();
        this.status.textContent = `${option.label} removed.`;
    }

    renderSelection() {
        this.chips.replaceChildren();
        this.selection.hidden = this.selected.size === 0;
        this.selectionCount.textContent = formatCount(
            this.selected.size,
            this.entitySingular,
            this.entityPlural,
        );
        this.valueInput.value = Array.from(this.selected.keys()).join(",");

        for (const option of this.selected.values()) {
            const chip = createElement("div", "entity-selector-chip");
            const copy = createElement("span", "entity-selector-chip-copy");
            copy.append(
                createElement("strong", "", option.label),
                createElement("code", "", option.identifier),
            );

            const removeButton = createElement("button", "entity-selector-chip-remove");
            removeButton.type = "button";
            removeButton.dataset.entityRemove = option.value;
            removeButton.setAttribute("aria-label", `Remove ${option.label} ${option.identifier}`);
            removeButton.textContent = "×";

            chip.append(copy, removeButton);
            this.chips.append(chip);
        }
    }
}

function createElement(tagName, className, text) {
    const element = document.createElement(tagName);
    if (className) {
        element.className = className;
    }
    if (text !== undefined) {
        element.textContent = text;
    }
    return element;
}

function createHighlightedText(value, query) {
    const fragment = document.createDocumentFragment();
    const matchStart = value.toLocaleLowerCase().indexOf(query.toLocaleLowerCase());

    if (!query || matchStart < 0) {
        fragment.append(document.createTextNode(value));
        return fragment;
    }

    const matchEnd = matchStart + query.length;
    fragment.append(document.createTextNode(value.slice(0, matchStart)));
    fragment.append(createElement("mark", "", value.slice(matchStart, matchEnd)));
    fragment.append(document.createTextNode(value.slice(matchEnd)));

    return fragment;
}

function normalizeEntityOption(option) {
    return {
        value: String(option.value),
        label: String(option.label),
        description: String(option.description ?? ""),
        identifier: String(option.identifier ?? option.value),
        badge: String(option.badge ?? ""),
        badgeTone: String(option.badge_tone ?? "neutral"),
        footnote: String(option.footnote ?? ""),
        group: option.group ? String(option.group) : "",
    };
}

function formatCount(count, singular, plural) {
    return `${count} ${count === 1 ? singular : plural}`;
}

function initializeEntitySelectors() {
    const selectors = Array.from(document.querySelectorAll("[data-entity-selector]"))
        .map((root) => new EntitySelector(root));

    for (const selector of selectors) {
        selector.connect();
    }

    document.addEventListener("keydown", (event) => {
        const target = event.target;
        const isTyping = target instanceof HTMLInputElement
            || target instanceof HTMLTextAreaElement
            || target instanceof HTMLSelectElement
            || target?.isContentEditable;

        if (event.key === "/" && !isTyping) {
            const visibleSelector = selectors.find((selector) => selector.root.offsetParent !== null);
            if (visibleSelector) {
                event.preventDefault();
                visibleSelector.focus();
            }
        }
    });
}

class CustomSelect {
    constructor(select, index) {
        this.select = select;
        this.index = index;
        this.wrapper = createElement("span", "custom-select");
        this.trigger = createElement("button", "custom-select-trigger");
        this.value = createElement("span", "custom-select-value");
        this.chevron = createElement("span", "select-chevron");
        this.popover = createElement("div", "custom-select-popover");
        this.options = [];
    }

    connect() {
        const identifier = `custom-select-${this.index}`;
        const label = this.select.getAttribute("aria-label")
            ?? this.select.closest("label")?.firstChild?.textContent?.trim()
            ?? this.select.name;

        this.select.classList.add("custom-select-native");
        this.select.tabIndex = -1;
        this.select.setAttribute("aria-hidden", "true");

        this.trigger.type = "button";
        this.trigger.role = "combobox";
        this.trigger.setAttribute("aria-label", label);
        this.trigger.setAttribute("aria-haspopup", "listbox");
        this.trigger.setAttribute("aria-expanded", "false");
        this.trigger.setAttribute("aria-controls", `${identifier}-options`);
        this.trigger.setAttribute("popovertarget", `${identifier}-popover`);
        this.chevron.setAttribute("aria-hidden", "true");
        this.trigger.append(this.value, this.chevron);

        this.popover.id = `${identifier}-popover`;
        this.popover.popover = "auto";
        this.popover.setAttribute("role", "listbox");
        this.popover.setAttribute("aria-label", label);

        const optionList = createElement("div", "custom-select-options");
        optionList.id = `${identifier}-options`;
        this.popover.append(optionList);

        for (const [optionIndex, option] of Array.from(this.select.options).entries()) {
            const button = createElement("button", "custom-select-option", option.textContent);
            button.type = "button";
            button.role = "option";
            button.disabled = option.disabled;
            button.dataset.value = option.value;
            button.dataset.optionIndex = String(optionIndex);
            button.addEventListener("click", () => this.choose(option.value));
            button.addEventListener("keydown", (event) => this.handleOptionKeydown(event));
            optionList.append(button);
            this.options.push(button);
        }

        this.select.before(this.wrapper);
        this.wrapper.append(this.select, this.trigger, this.popover);
        this.select.addEventListener("change", () => this.render());
        this.trigger.addEventListener("keydown", (event) => this.handleTriggerKeydown(event));
        this.popover.addEventListener("beforetoggle", (event) => {
            const isOpening = event.newState === "open";
            this.trigger.setAttribute("aria-expanded", String(isOpening));
            this.wrapper.classList.toggle("is-open", isOpening);
            if (isOpening) {
                this.positionPopover();
            }
        });
        this.popover.addEventListener("toggle", (event) => {
            if (event.newState === "open") {
                this.selectedButton()?.focus();
            }
        });

        this.render();
    }

    choose(value) {
        this.select.value = value;
        this.select.dispatchEvent(new Event("change", { bubbles: true }));
        this.popover.hidePopover();
        this.trigger.focus();
    }

    render() {
        const selected = this.select.selectedOptions[0];
        this.value.textContent = selected?.textContent ?? "Select";

        for (const option of this.options) {
            const isSelected = option.dataset.value === this.select.value;
            option.classList.toggle("is-selected", isSelected);
            option.setAttribute("aria-selected", String(isSelected));
        }
    }

    selectedButton() {
        return this.options.find((option) => option.dataset.value === this.select.value)
            ?? this.options[0];
    }

    positionPopover() {
        const bounds = this.trigger.getBoundingClientRect();
        const availableBelow = window.innerHeight - bounds.bottom - 12;
        const openAbove = availableBelow < 180 && bounds.top > availableBelow;

        this.popover.style.width = `${bounds.width}px`;
        this.popover.style.left = `${Math.min(bounds.left, window.innerWidth - bounds.width - 8)}px`;
        this.popover.style.top = openAbove ? "auto" : `${bounds.bottom + 6}px`;
        this.popover.style.bottom = openAbove
            ? `${window.innerHeight - bounds.top + 6}px`
            : "auto";
    }

    handleTriggerKeydown(event) {
        if (!["ArrowDown", "ArrowUp", "Enter", " "].includes(event.key)) {
            return;
        }

        event.preventDefault();
        this.popover.showPopover();
    }

    handleOptionKeydown(event) {
        const currentIndex = Number(event.currentTarget.dataset.optionIndex);
        if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            event.preventDefault();
            const direction = event.key === "ArrowDown" ? 1 : -1;
            const nextIndex = Math.max(0, Math.min(currentIndex + direction, this.options.length - 1));
            this.options[nextIndex].focus();
        } else if (event.key === "Escape") {
            this.popover.hidePopover();
            this.trigger.focus();
        }
    }
}

function initializeSelectControls() {
    for (const [index, select] of document.querySelectorAll("select").entries()) {
        new CustomSelect(select, index).connect();
    }
}

function initializeFieldOrdering() {
    const orderForm = document.querySelector("[data-field-order-form]");
    if (!orderForm) {
        return;
    }

    const orderList = orderForm.querySelector("[data-field-order-list]");
    const orderStatus = orderForm.querySelector("[data-field-order-status]");
    let draggedRow = null;
    let saveQueue = Promise.resolve();

    const rows = () => Array.from(orderList.querySelectorAll("[data-field-row]"));

    const moveDraggedRow = (event) => {
        const hoveredRow = event.target.closest("[data-field-row]");
        if (!draggedRow || !hoveredRow || hoveredRow === draggedRow) {
            return;
        }

        const bounds = hoveredRow.getBoundingClientRect();
        const insertBefore = event.clientY < bounds.top + bounds.height / 2;
        orderList.insertBefore(draggedRow, insertBefore ? hoveredRow : hoveredRow.nextSibling);
    };

    const saveOrder = (movedRow) => {
        const orderedRows = rows();
        const keys = orderedRows.map((row) => row.dataset.fieldKey).join(",");
        const label = movedRow.querySelector("td:nth-child(3)").textContent.trim();
        const position = orderedRows.indexOf(movedRow) + 1;
        const body = new URLSearchParams({ csrf: orderForm.dataset.csrf, keys });

        orderStatus.classList.remove("is-error");
        orderStatus.textContent = `Saving ${label} at position ${position}…`;
        saveQueue = saveQueue
            .catch(() => {})
            .then(async () => {
                const response = await fetch(orderForm.dataset.orderUrl, {
                    method: "POST",
                    headers: {
                        Accept: "application/json",
                        "Content-Type": "application/x-www-form-urlencoded",
                    },
                    body,
                });

                if (!response.ok) {
                    throw new Error(`Saving field order returned ${response.status}`);
                }

                orderStatus.textContent = `${label} is now at position ${position}.`;
            })
            .catch(() => {
                orderStatus.classList.add("is-error");
                orderStatus.textContent = "The field order could not be saved. Reload and try again.";
            });
    };

    orderList.addEventListener("dragstart", (event) => {
        const row = event.target.closest("[data-field-row]");
        if (!row) {
            return;
        }

        draggedRow = row;
        draggedRow.classList.add("is-dragging");
        event.dataTransfer.effectAllowed = "move";
        event.dataTransfer.setData("text/plain", row.dataset.fieldKey);
    });

    orderList.addEventListener("dragover", (event) => {
        if (!draggedRow) {
            return;
        }

        event.preventDefault();
        moveDraggedRow(event);
    });

    orderList.addEventListener("drop", (event) => {
        event.preventDefault();
        moveDraggedRow(event);
        saveOrder(draggedRow);
    });

    orderList.addEventListener("dragend", () => {
        draggedRow?.classList.remove("is-dragging");
        draggedRow = null;
    });

    orderList.addEventListener("keydown", (event) => {
        const handle = event.target.closest(".drag-handle");
        if (!handle || !event.altKey || !["ArrowUp", "ArrowDown"].includes(event.key)) {
            return;
        }

        const row = handle.closest("[data-field-row]");
        const sibling = event.key === "ArrowUp"
            ? row.previousElementSibling
            : row.nextElementSibling;
        if (!sibling) {
            return;
        }

        event.preventDefault();
        if (event.key === "ArrowUp") {
            orderList.insertBefore(row, sibling);
        } else {
            orderList.insertBefore(sibling, row);
        }

        saveOrder(row);
        handle.focus();
    });
}

function initializeSettingsHelp() {
    const editor = document.querySelector("[data-settings-editor]");
    const help = document.querySelector("[data-settings-help]");
    if (!editor || !help) {
        return;
    }

    const empty = help.querySelector("[data-settings-help-empty]");
    const entries = Array.from(help.querySelectorAll("[data-setting-help-entry]"));

    const update = () => {
        const lineStart = editor.value.lastIndexOf("\n", editor.selectionStart - 1) + 1;
        const lineEnd = editor.value.indexOf("\n", editor.selectionStart);
        const line = editor.value.slice(lineStart, lineEnd < 0 ? undefined : lineEnd);
        const currentKey = line.match(/"([A-Za-z0-9._-]+)"\s*:/)?.[1];

        empty.hidden = Boolean(currentKey);
        let matched = false;
        for (const entry of entries) {
            const active = entry.dataset.settingKey === currentKey;
            entry.hidden = !active;
            matched ||= active;
        }
        empty.hidden = matched;
    };

    for (const eventName of ["click", "keyup", "select", "input", "focus"]) {
        editor.addEventListener(eventName, update);
    }
}

function initializeAutoFilters() {
    for (const form of document.querySelectorAll("[data-auto-filter]")) {
        let submitTimer = null;

        const submit = () => {
            clearTimeout(submitTimer);
            form.requestSubmit();
        };

        form.addEventListener("input", (event) => {
            if (!(event.target instanceof HTMLInputElement)) {
                return;
            }

            clearTimeout(submitTimer);
            submitTimer = setTimeout(submit, FILTER_SUBMIT_DELAY_MS);
        });
        form.addEventListener("change", submit);
    }
}

initializeEntitySelectors();
initializeSelectControls();
initializeFieldOrdering();
initializeSettingsHelp();
initializeAutoFilters();
