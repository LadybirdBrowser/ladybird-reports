const ENTITY_SEARCH_DELAY_MS = 180;

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
        this.popover.hidden = false;
        this.searchInput.setAttribute("aria-expanded", "true");
    }

    close() {
        this.popover.hidden = true;
        this.searchInput.setAttribute("aria-expanded", "false");
        this.searchInput.removeAttribute("aria-activedescendant");
        this.activeIndex = -1;
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

        for (const [index, option] of this.options.entries()) {
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
        if (this.popover.hidden) {
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

function initializeSelectControls() {
    for (const select of document.querySelectorAll("select")) {
        if (select.parentElement?.classList.contains("select-control")) {
            continue;
        }

        const wrapper = document.createElement("span");
        wrapper.className = "select-control";
        select.before(wrapper);
        wrapper.append(select);
    }
}

function initializeFieldOrdering() {
    const orderForm = document.querySelector("[data-field-order-form]");
    if (!orderForm) {
        return;
    }

    const orderList = orderForm.querySelector("[data-field-order-list]");
    const orderInput = orderForm.querySelector("[data-field-order-input]");
    const orderStatus = orderForm.querySelector("[data-field-order-status]");
    let draggedRow = null;

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

    const updateOrder = (movedRow) => {
        const orderedRows = rows();
        orderInput.value = orderedRows.map((row) => row.dataset.fieldKey).join(",");

        for (const [index, row] of orderedRows.entries()) {
            row.querySelector('[data-move="up"]').disabled = index === 0;
            row.querySelector('[data-move="down"]').disabled = index === orderedRows.length - 1;
        }

        if (movedRow) {
            const label = movedRow.querySelector("td:nth-child(3)").textContent.trim();
            const position = orderedRows.indexOf(movedRow) + 1;
            orderStatus.textContent = `${label} moved to position ${position}. Save to apply.`;
        }
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
        updateOrder(draggedRow);
    });

    orderList.addEventListener("dragend", () => {
        draggedRow?.classList.remove("is-dragging");
        updateOrder(draggedRow);
        draggedRow = null;
    });

    orderList.addEventListener("click", (event) => {
        const button = event.target.closest("[data-move]");
        if (!button) {
            return;
        }

        const row = button.closest("[data-field-row]");
        if (button.dataset.move === "up" && row.previousElementSibling) {
            orderList.insertBefore(row, row.previousElementSibling);
        } else if (button.dataset.move === "down" && row.nextElementSibling) {
            orderList.insertBefore(row.nextElementSibling, row);
        }

        updateOrder(row);
        row.focus();
    });

    updateOrder();
}

initializeEntitySelectors();
initializeSelectControls();
initializeFieldOrdering();
