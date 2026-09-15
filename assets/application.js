const orderForm = document.querySelector("[data-field-order-form]");

if (orderForm) {
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
