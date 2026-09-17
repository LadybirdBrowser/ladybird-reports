import { expect, test } from "@playwright/test";
import { createHmac } from "node:crypto";

const reportId = "01a0a536-01cd-7ac7-a3cf-ae6a2d5030e5";

test("unauthenticated visitors can only reach the sign-in page", async ({ page }) => {
  await page.setExtraHTTPHeaders({ "x-request-id": "caller-controlled" });
  const response = await page.goto("/");

  expect(response?.status()).toBe(200);
  expect(response?.headers()["x-request-id"]).toMatch(
    /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
  );
  await expect(page).toHaveURL(/\/login$/);
  await expect(page.getByRole("heading", { name: "Welcome to Reports" })).toBeVisible();
  await expect(page.getByText("REPORTING_DATABASE_URL")).toHaveCount(0);
  await expect(page.locator(".login-emblem img")).toHaveAttribute(
    "src",
    "/assets/ladybird-mark.png",
  );
  expect(
    await page.locator(".login-emblem img").evaluate(
      (image: HTMLImageElement) => image.naturalWidth === image.naturalHeight,
    ),
  ).toBe(true);

  const visibleText = await page.locator("body").innerText();
  expect(visibleText).not.toMatch(/maintainer|LadybirdBrowser\/maintainers|secure/i);
  await expect(page.locator("footer")).toContainText("Ladybird Reports · 0.1.0-dev");
});

test("sign-in returns to the requested report and preserves its query", async ({ page }) => {
  const destination = `/reports/${reportId}?source=discord&view=raw`;
  await page.goto(destination);

  const loginUrl = new URL(page.url());
  expect(loginUrl.pathname).toBe("/login");
  expect(loginUrl.searchParams.get("next")).toBe(destination);

  const loginLink = page.getByRole("link", { name: "Continue with GitHub" });
  const githubLoginUrl = new URL(await loginLink.getAttribute("href")!, loginUrl);
  expect(githubLoginUrl.pathname).toBe("/auth/github");
  expect(githubLoginUrl.searchParams.get("next")).toBe(destination);

  await loginLink.click();
  await expect(page).toHaveURL(new RegExp(`/reports/${reportId}\\?source=discord&view=raw$`));
  await expect(page.getByRole("heading", { name: "Stack trace" })).toBeVisible();
  expect((await page.context().cookies()).some((cookie) => cookie.name === "oauth_state"))
    .toBe(false);
});

test("sign-in ignores an external return destination", async ({ page }) => {
  await page.goto("/login?next=%2F%2Fevil.example%2Fpath");
  await expect(page.getByRole("link", { name: "Continue with GitHub" }))
    .toHaveAttribute("href", "/auth/github");
});

test.describe("authenticated management UI", () => {
  test.use({
    storageState: "target/browser-test-storage-state.json",
  });

  test("shows setup credentials and escapes unknown report fields", async ({ page }) => {
    const response = await page.goto("/");

    expect(response?.headers()["x-request-id"]).toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
    );
    await expect(page.getByText("Reporting database setup is incomplete.")).toBeVisible();
    const reportLink = page.locator(`a[href="/reports/${reportId}"]`);
    await expect(reportLink).toHaveText(
      "Web compatibility: WebContent::ConnectionFromClient::debug_request",
    );
    await expect(page.locator("tbody")).not.toContainText(reportId);
    await expect(reportLink.locator("xpath=ancestor::tr"))
      .toContainText(/\d{2} [A-Z][a-z]{2} \d{4}, \d{2}:\d{2} UTC/);

    await reportLink.click();
    await expect(page.getByRole("heading", {
      name: "Web compatibility: WebContent::ConnectionFromClient::debug_request",
    })).toBeVisible();
    await expect(page.getByRole("heading", { name: "Stack trace" })).toBeVisible();
    await expect(page.getByRole("rowheader", { name: /Git commit/ })).toBeVisible();
    await expect(page.getByText("654cf9b187384fa8855eac4fbafaa70e75497083"))
      .toBeVisible();
    await expect(page.getByRole("rowheader", { name: /Build configuration/ })).toBeVisible();
    await expect(page.getByText("AppleClang 21.0.0.21000101")).toBeVisible();
    const diagnosticTable = page.locator(".report-field-table").first();
    await expect(diagnosticTable.getByRole("rowheader", { name: /Git commit/ }))
      .toBeVisible();
    await expect(diagnosticTable.getByRole("rowheader", { name: /C\+\+ compiler/ }))
      .toBeVisible();
    await page.setViewportSize({ width: 390, height: 800 });
    expect(await page.locator(".stack-frame-symbol").first().evaluate(
      (cell) => cell.getBoundingClientRect().width,
    )).toBeGreaterThan(200);
    expect(await page.evaluate(() => document.documentElement.scrollWidth))
      .toBeLessThanOrEqual(390);
    await page.setViewportSize({ width: 1280, height: 800 });
    const stackHeader = page.locator(".report-field-header").filter({
      has: page.getByRole("heading", { name: "Stack trace" }),
    });
    await expect(stackHeader.locator(".stack-signature")).toBeVisible();
    await expect(stackHeader.getByLabel("Show raw")).toBeVisible();
    expect(await stackHeader.evaluate((header) => {
      const title = header.querySelector("h3")!.getBoundingClientRect();
      const actions = header.querySelector(".stack-header-actions")!.getBoundingClientRect();
      return Math.abs(title.top - actions.top);
    })).toBeLessThan(12);
    await expect(page.locator(".stack-frame-table").getByRole("row").first()).toBeVisible();
    await expect(page.getByText("Core::ThreadEventQueue::process()", { exact: true })).toBeVisible();
    await expect(page.getByRole("heading", { name: "Possible matches" })).toBeVisible();
    const possibleMatch = page.locator(".similar-report").filter({
      hasText: "Intermittent navigation timeout",
    });
    await expect(possibleMatch.locator(".similar-report-main"))
      .toHaveAttribute("href", /^\/reports\//);
    await expect(possibleMatch).toContainText(/\d{2} \w{3} \d{4}, \d{2}:\d{2} UTC/);
    await possibleMatch.scrollIntoViewIfNeeded();
    expect(await possibleMatch.evaluate((row) => {
      const bounds = row.getBoundingClientRect();
      return document.elementFromPoint(bounds.left + 6, bounds.top + 6)
        ?.closest("a")?.classList.contains("similar-report-main");
    })).toBe(true);
    await expect(page.getByText("Exact signature")).toBeVisible();
    await expect(page.getByRole("button", { name: "Add to issue" })).toBeVisible();
    await expect(page.locator(".badge-triage")).toHaveText("Needs triage");
    await page.getByLabel("Show raw").check();
    await expect(page.locator(".stack-raw-text"))
      .toContainText("Native stack (binary build ID, object address):");
    await expect(page.locator(".stack-frame-table")).toBeHidden();
    await page.getByLabel("Show raw").uncheck();
    await expect(page.locator(".stack-frame-table")).toBeVisible();
    await expect(page.getByText("macOS", { exact: true }).first()).toBeVisible();
    await expect(page.getByText("arm64", { exact: true }).first()).toBeVisible();
    await expect(page.getByText("127.0.0.1", { exact: true })).toBeVisible();
    const sourceAddress = page.locator(".definition-list > div").filter({
      has: page.getByText("Source IP address", { exact: true }),
    });
    await sourceAddress.locator("code").evaluate((value) => {
      value.textContent = "2001:9e0:862a:6201:d8fd:5d40:445d:4115";
    });
    expect(await sourceAddress.evaluate((field) => field.scrollWidth <= field.clientWidth))
      .toBe(true);
    await expect(page.getByRole("heading", { name: "Additional fields" })).toBeVisible();
    await expect(page.locator(".report-field-value").filter({ hasText: "<script>" })).toBeVisible();
    await expect(page.getByRole("link", { name: "Filter reports by Stack trace" })).toHaveCount(0);
    await expect(page.getByRole("heading", { name: "Attachments" }).locator(".."))
      .toContainText("1 file");
    const attachmentRow = page.locator(".attachment-table tbody tr").first();
    await expect(attachmentRow).toContainText("crash-diagnostics.txt");
    await expect(attachmentRow).toContainText("text/plain");
    await expect(attachmentRow).toContainText("3.7 KiB");
    const downloadLink = attachmentRow.locator("td:nth-child(2) a");
    const attachmentResponse = await page.request.get(await downloadLink.getAttribute("href")!);
    expect(attachmentResponse.headers()["content-disposition"])
      .toContain('attachment; filename="crash-diagnostics.txt"');
    const download = page.waitForEvent("download");
    await downloadLink.click();
    expect((await download).suggestedFilename()).toBe("crash-diagnostics.txt");
    expect(await page.getByRole("heading", { name: "Attachments" }).evaluate((attachments) => {
      const additionalFields = document.querySelector("#additional-fields-title");
      return Boolean(additionalFields &&
        attachments.compareDocumentPosition(additionalFields) & Node.DOCUMENT_POSITION_FOLLOWING);
    })).toBe(true);

    const stackTrace = page.locator(".stack-trace");
    expect(
      await stackTrace.evaluate((element) => element.scrollWidth <= element.clientWidth),
    ).toBe(true);

    await page.getByRole("region", { name: "Overview" })
      .getByRole("link", { name: "Filter reports by Platform" })
      .click();
    expect(new URL(page.url()).searchParams.get("q")).toBe("state:all platform:macos");
    await expect(page.locator(`a[href="/reports/${reportId}"]`)).toBeVisible();

    expect(await page.evaluate(() => (window as any).fixtureWasExecuted)).toBeUndefined();
  });

  test("keeps report rows readable on desktop and mobile", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto("/");
    const row = page.locator(".report-table tbody tr").first();
    const title = row.locator(".report-title-link");
    const metadata = row.locator(".report-table-metadata");
    await expect(metadata).toContainText("macOS · arm64 · 0.1.0-browser-test");
    const desktopTitle = await title.boundingBox();
    const desktopMetadata = await metadata.boundingBox();
    expect(desktopTitle).not.toBeNull();
    expect(desktopMetadata).not.toBeNull();
    expect(desktopMetadata!.y).toBeGreaterThan(desktopTitle!.y);

    for (const width of [390, 320]) {
      await page.setViewportSize({ width, height: 800 });
      const report = await row.locator(".report-table-report").boundingBox();
      const state = await row.locator(".report-table-state").boundingBox();
      const received = await row.locator(".report-table-received").boundingBox();
      expect(report).not.toBeNull();
      expect(state).not.toBeNull();
      expect(received).not.toBeNull();
      expect(state!.y).toBeGreaterThan(report!.y + report!.height);
      expect(received!.x).toBeGreaterThan(state!.x);
      expect(report!.width).toBeGreaterThan(220);
      expect(await page.evaluate(() => document.documentElement.scrollWidth))
        .toBeLessThanOrEqual(width);
    }
  });

  test("keeps legacy build details on a complete final overview row", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto("/");

    const legacyReport = page.locator(".report-table tbody tr")
      .filter({ hasText: "macOS · arm64 · Ladybird Nightly 2026-09-15" }).first();
    await legacyReport.getByRole("link", {
      name: "Crash: WebContent::ConnectionFromClient::debug_request",
    }).click();

    const fields = page.locator(".definition-list > div");
    await expect(fields).toHaveCount(7);
    await expect(fields.nth(4).locator("dt")).toHaveText("Submitted");
    await expect(fields.nth(5).locator("dt")).toHaveText("Source IP address");
    await expect(fields.nth(6).locator("dt")).toHaveText("Build");

    const submitted = await fields.nth(4).boundingBox();
    const sourceIp = await fields.nth(5).boundingBox();
    const build = await fields.nth(6).boundingBox();
    expect(submitted).not.toBeNull();
    expect(sourceIp).not.toBeNull();
    expect(build).not.toBeNull();
    expect(sourceIp!.x).toBeGreaterThan(submitted!.x);
    expect(build!.width).toBeGreaterThan(submitted!.width + sourceIp!.width - 2);
    expect(await fields.nth(5).evaluate((field) =>
      getComputedStyle(field).borderBottomWidth,
    )).toBe("1px");

    await page.setViewportSize({ width: 390, height: 800 });
    expect(await page.evaluate(() => document.documentElement.scrollWidth))
      .toBeLessThanOrEqual(390);
  });

  test("sets security headers and revalidates static assets", async ({ page, request }) => {
    const pageResponse = await page.goto("/settings");
    const pageHeaders = pageResponse?.headers() ?? {};

    expect(pageHeaders["cache-control"]).toBe("private, no-store");
    expect(pageHeaders["content-security-policy"]).toContain("default-src 'none'");
    expect(pageHeaders["content-security-policy"]).toContain("script-src 'self'");
    expect(pageHeaders["content-security-policy"]).toContain("connect-src 'self'");
    expect(pageHeaders["cross-origin-opener-policy"]).toBe("same-origin");
    expect(pageHeaders["cross-origin-resource-policy"]).toBe("same-origin");
    expect(pageHeaders["permissions-policy"]).toContain("camera=()");
    expect(pageHeaders["referrer-policy"]).toBe("no-referrer");
    expect(pageHeaders["x-content-type-options"]).toBe("nosniff");
    expect(pageHeaders["x-frame-options"]).toBe("DENY");

    for (const asset of ["application.css", "application.js", "reports.js"]) {
      const firstResponse = await request.get(`/assets/${asset}`);
      const etag = firstResponse.headers()["etag"];

      expect(firstResponse.status()).toBe(200);
      expect(firstResponse.headers()["cache-control"]).toBe(
        "public, max-age=0, must-revalidate",
      );
      expect(etag).toMatch(/^\"[0-9a-f]{64}\"$/);

      const revalidatedResponse = await request.get(`/assets/${asset}`, {
        headers: { "if-none-match": etag },
      });

      expect(revalidatedResponse.status()).toBe(304);
      expect(revalidatedResponse.headers()["etag"]).toBe(etag);
    }

    const markResponse = await request.get("/assets/ladybird-mark.png");
    const markEtag = markResponse.headers()["etag"];

    expect(markResponse.status()).toBe(200);
    expect(markResponse.headers()["content-type"]).toBe("image/png");
    expect(markResponse.headers()["cache-control"]).toBe(
      "public, max-age=0, must-revalidate",
    );
    expect(markEtag).toMatch(/^\"[0-9a-f]{64}\"$/);

    const revalidatedMark = await request.get("/assets/ladybird-mark.png", {
      headers: { "if-none-match": markEtag },
    });

    expect(revalidatedMark.status()).toBe(304);
    expect(revalidatedMark.headers()["etag"]).toBe(markEtag);
  });

  test("searches tracked and GitHub issues in one selector", async ({ page }) => {
    await page.goto("/issues");
    await expect(page.getByText("Intermittent navigation timeout")).toBeVisible();

    await page.getByLabel("Include resolved").check();
    await expect.poll(() => new URL(page.url()).searchParams.get("resolved"))
      .toBe("true");

    await page.goto(`/reports/${reportId}`);
    await page.getByRole("button", { name: "Add report to issue" }).click();
    const issueDialog = page.getByRole("dialog", { name: "Add report to an issue" });
    await expect(issueDialog).toBeVisible();
    await expect(issueDialog).toHaveClass(/modal-overlay/);
    expect(
      await issueDialog.evaluate(
        (element) => getComputedStyle(element, "::backdrop").backdropFilter,
      ),
    ).toBe("blur(2px)");
    const issueSearch = page.getByRole("combobox", { name: "Search issues" });
    await issueSearch.fill("navigation");

    const issuePopover = page
      .locator('[data-search-url="/api/issue-options"]')
      .locator("[data-entity-popover]");
    await expect(issuePopover).toBeVisible();
    expect(await issuePopover.evaluate((element) => element.matches(":popover-open"))).toBe(true);

    const [popoverBounds, dialogBounds] = await Promise.all([
      issuePopover.boundingBox(),
      page.getByRole("dialog", { name: "Add report to an issue" }).boundingBox(),
    ]);
    expect(popoverBounds).not.toBeNull();
    expect(dialogBounds).not.toBeNull();
    expect(popoverBounds!.y + popoverBounds!.height).toBeLessThanOrEqual(
      page.viewportSize()!.height,
    );

    await expect(issuePopover.getByText("Tracked issues", { exact: true })).toBeVisible();
    await expect(issuePopover.getByText("GitHub issues", { exact: true })).toBeVisible();

    const trackedIssue = page.getByRole("option", {
      name: /Intermittent navigation timeout/,
    });
    await expect(trackedIssue).toBeVisible();
    await expect(trackedIssue).toContainText("#4812");
    await expect(issuePopover.getByText("#4812", { exact: true })).toHaveCount(1);

    const githubIssue = page.getByRole("option", {
      name: /Investigate renderer overlap/,
    });
    await expect(githubIssue).toContainText("Not yet tracked in Reports");
    await githubIssue.click();

    await expect(page.locator(".entity-selector-chip")).toContainText(
      "Investigate renderer overlap",
    );
    await expect(issueDialog.locator('input[name="issue_selection"]'))
      .toHaveValue("github:6200");

    await page.getByRole("button", { name: "Create new" }).click();
    await expect(page.getByLabel("Title", { exact: true }))
      .toHaveValue("Web compatibility: WebContent::ConnectionFromClient::debug_request");
    await expect(page.getByLabel("Description", { exact: true }))
      .toHaveValue(/- Platform: macOS/);
    await expect(page.getByLabel("Description", { exact: true }))
      .toHaveValue(/## Stack trace\n\n```text\n[\s\S]*Core::ThreadEventQueue::process\(\)/);
    await expect(page.getByText("Review this public GitHub issue"))
      .toBeVisible();
  });

  test("searches existing internal issues in the issue workflow", async ({ page }) => {
    await page.goto("/?q=state%3Atriage+platform%3APaginationOS");
    await page.locator(".report-title-link").first().click();
    await page.getByRole("button", { name: "Add report to issue" }).click();

    const issueSearch = page.getByRole("combobox", { name: "Search issues" });
    await issueSearch.fill("navigation timeout");
    const option = page.getByRole("option", { name: /Intermittent navigation timeout/ });
    await expect(option).toBeVisible();
    await expect(option).toContainText("GitHub issue #4812");
    await option.click();

    await expect(page.getByRole("dialog", { name: "Add report to an issue" })
      .locator('input[name="issue_selection"]'))
      .toHaveValue(/^issue:[0-9a-f-]+$/);
    await expect(page.locator(".entity-selector-chip")).toContainText(
      "Intermittent navigation timeout",
    );
    await page.getByRole("dialog", { name: "Add report to an issue" })
      .getByRole("button", { name: "Add report", exact: true })
      .click();
    await expect(page).toHaveURL(/\/issues\/[0-9a-f-]+$/);
    await expect(page.getByRole("heading", { name: "Intermittent navigation timeout" }))
      .toBeVisible();
  });

  test("searches reports with qualified syntax", async ({ page }) => {
    await page.goto("/");
    await expect(page.getByLabel("Search reports"))
      .toHaveValue("state:triage state:confirmed");
    await page.evaluate(() => ((window as any).reportPageStayedLoaded = true));
    const search = page.getByLabel("Search reports");
    const initialUrl = page.url();
    await search.fill("state:triage platform:macos kind:crash");
    await page.waitForTimeout(800);

    expect(page.url()).toBe(initialUrl);
    await search.press("Enter");

    await expect.poll(() => new URL(page.url()).searchParams.get("q"))
      .toBe("state:triage platform:macos kind:crash");
    expect(await page.evaluate(() => (window as any).reportPageStayedLoaded)).toBe(true);
    await expect(search).toBeFocused();
    await expect(page.locator("tbody tr")).toHaveCount(2);
    await expect(page.locator("tbody tr").nth(0)).toContainText("macOS");
    await expect(page.locator("tbody tr").nth(1)).toContainText("macOS");

    await search.fill("platform:linux");
    await search.blur();
    await expect.poll(() => new URL(page.url()).searchParams.get("q"))
      .toBe("platform:linux");
    await expect(page.locator("tbody tr")).toHaveCount(2);

    await search.fill("plat");
    await expect(page.getByRole("option", { name: /^platform:/ })).toBeVisible();
    await search.fill("state:triage platform:macos platform:linux");
    await search.press("Enter");
    await expect.poll(() => new URL(page.url()).searchParams.get("q"))
      .toBe("state:triage platform:macos platform:linux");
    await expect(page.locator("tbody tr")).toHaveCount(4);
  });

  test("defaults to triage and confirmed reports", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator(".badge-triage").first()).toBeVisible();
    await expect(page.locator(".badge-confirmed").first()).toHaveText("Confirmed");
    await expect(page.locator(".badge-rejected")).toHaveCount(0);

    const search = page.getByLabel("Search reports");
    await search.fill("");
    await search.blur();

    await expect.poll(() => new URL(page.url()).searchParams.get("q")).toBe("");
    await expect(page.locator(".badge-confirmed").first()).toHaveText("Confirmed");
  });

  test("applies an autocomplete value with Enter", async ({ page }) => {
    await page.goto("/");
    const search = page.getByLabel("Search reports");
    await search.fill("plat");
    await expect(page.getByRole("option", { name: /^platform:/ })).toBeVisible();
    await search.press("ArrowDown");
    await search.press("Enter");
    await expect(search).toHaveValue("platform:");

    await search.pressSequentially("ma");
    await expect(page.getByRole("option", { name: /^macOS/ })).toBeVisible();
    await search.press("ArrowDown");
    await search.press("Enter");
    await expect(search).toHaveValue("platform:macos");
    await expect.poll(() => new URL(page.url()).searchParams.get("q"))
      .toBe("platform:macos");

    await search.fill("stack:Core");
    await expect.poll(() => search.getAttribute("aria-expanded")).toBe("false");
  });

  test("loads reports fifty at a time without navigating", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator("tbody tr")).toHaveCount(50);
    const initialUrl = page.url();
    await page.getByRole("button", { name: "Show more…" }).click();
    await expect(page.locator("tbody tr")).toHaveCount(58);
    expect(page.url()).toBe(initialUrl);
    await expect(page.getByRole("button", { name: "Show more…" })).toHaveCount(0);
  });

  test("blocks and unblocks the submission source from the report actions", async ({ page }) => {
    await page.goto(`/reports/${reportId}`);

    await expect(page.getByRole("heading", { name: "Actions" })).toBeVisible();
    await page.getByRole("button", { name: "Block submission IP" }).click();
    const confirmation = page.getByRole("dialog", { name: "Block this IP?" });
    await expect(confirmation).toBeVisible();
    await expect(confirmation).toHaveClass(/modal-overlay/);
    expect(
      await confirmation.evaluate(
        (element) => getComputedStyle(element, "::backdrop").backdropFilter,
      ),
    ).toBe("blur(2px)");
    await expect(confirmation.getByRole("checkbox")).toBeChecked();
    await confirmation.getByRole("button", { name: "Cancel" }).click();
    await expect(confirmation).toBeHidden();

    await page.getByRole("button", { name: "Block submission IP" }).click();
    await confirmation.getByRole("checkbox").uncheck();
    await confirmation.getByRole("button", { name: "Block IP" }).click();
    await expect(page.getByText("IP is blocked indefinitely")).toBeVisible();
    await expect(page.locator(".badge-rejected")).toHaveText("Rejected");

    await page.getByRole("button", { name: "Unblock submission IP" }).click();
    await expect(page.getByRole("button", { name: "Block submission IP" })).toBeVisible();
    await page.getByRole("button", { name: "Restore report" }).click();
  });

  test("reorders recognized fields by dragging rows", async ({ page }) => {
    await page.goto("/settings");


    const stackRow = page.locator('[data-field-key="stack"]');
    const signalRow = page.locator('[data-field-key="signal"]');
    const signalBounds = await signalRow.boundingBox();
    expect(signalBounds).not.toBeNull();

    const dataTransfer = await page.evaluateHandle(() => new DataTransfer());
    await stackRow.locator(".drag-handle").dispatchEvent("dragstart", { dataTransfer });
    await signalRow.dispatchEvent("dragover", {
      dataTransfer,
      clientY: signalBounds!.y + signalBounds!.height - 1,
    });
    await signalRow.dispatchEvent("drop", {
      dataTransfer,
      clientY: signalBounds!.y + signalBounds!.height - 1,
    });
    await stackRow.locator(".drag-handle").dispatchEvent("dragend", { dataTransfer });

    const orderedRows = page.locator("[data-field-row]");
    await expect(orderedRows.nth(0)).toContainText("signal");
    await expect(orderedRows.nth(1)).toContainText("stack");

    await expect(page.getByText("Stack trace is now at position 2.")).toBeVisible();
    await page.reload();
    await expect(orderedRows.nth(0)).toContainText("signal");
    await expect(orderedRows.nth(1)).toContainText("stack");
  });

  test("explains the runtime setting under the caret", async ({ page }) => {
    await page.goto("/settings");

    const editor = page.getByLabel("Configuration JSON");
    await editor.evaluate((element: HTMLTextAreaElement) => {
      const offset = element.value.indexOf('"staging_retention_seconds"');
      element.focus();
      element.setSelectionRange(offset, offset);
      element.dispatchEvent(new Event("select", { bubbles: true }));
    });

    await expect(page.getByRole("heading", { name: "Staging retention" })).toBeVisible();
    await expect(page.getByText("Seconds since the staging directory was last modified.")).toBeVisible();
    await expect(page.getByText("Select a setting", { exact: true })).toBeHidden();

    await editor.evaluate((element: HTMLTextAreaElement) => {
      const offset = element.value.indexOf('"github_authorization_team"');
      element.setSelectionRange(offset, offset);
      element.dispatchEvent(new Event("select", { bubbles: true }));
    });
    await expect(page.getByRole("heading", { name: "GitHub access team" })).toBeVisible();
    await expect(editor).toHaveValue(/"github_authorization_team": "LadybirdBrowser\/maintainers"/);
  });

  test("opens account actions in a popover", async ({ page }) => {
    await page.goto("/");

    await page.getByRole("button", { name: "Open account menu for browser-tester" }).click();
    const signOut = page.getByRole("button", { name: "Sign out" });
    await expect(signOut).toBeVisible();
    expect(await signOut.evaluate((button) => getComputedStyle(button).borderTopWidth)).toBe("0px");
  });

  test("rejects and restores reports while retaining their details", async ({ page }) => {
    await page.goto("/?q=state%3Atriage+platform%3APaginationOS");
    const reportLink = page.locator(".report-title-link").first();
    const href = await reportLink.getAttribute("href");
    expect(href).not.toBeNull();
    await reportLink.click();

    await expect(page.locator(".badge-triage")).toHaveText("Needs triage");

    await page.getByRole("button", { name: "Reject report" }).first().click();
    const confirmation = page.getByRole("dialog", { name: "Reject this report?" });
    await expect(confirmation).toBeVisible();
    await confirmation.getByRole("button", { name: "Cancel" }).click();
    await expect(page.locator(".badge-triage")).toBeVisible();
    await page.getByRole("button", { name: "Reject report" }).first().click();
    await confirmation.getByRole("button", { name: "Reject report" }).click();
    await expect(page.locator(".badge-rejected")).toHaveText("Rejected");
    await page.getByRole("button", { name: "Restore report" }).click();
    await expect(page.locator(".badge-triage")).toHaveText("Needs triage");
    await page.getByRole("button", { name: "Reject report" }).first().click();
    await confirmation.getByRole("button", { name: "Reject report" }).click();
    await expect(page.locator(".badge-rejected")).toHaveText("Rejected");
    await expect(page).toHaveURL(new RegExp(`${href}$`));

    await page.getByRole("link", { name: "Reports", exact: true }).click();
    await expect(page.locator(`a[href="${href}"]`)).toHaveCount(0);
    await page.getByLabel("Search reports").fill("state:rejected platform:PaginationOS");
    await page.getByLabel("Search reports").press("Enter");
    await expect.poll(() => new URL(page.url()).searchParams.get("q"))
      .toBe("state:rejected platform:PaginationOS");
    await expect(page.locator(`a[href="${href}"]`)).toBeVisible();
  });

  test("creates an issue, assigns the report, and filters it from triage", async ({ page }) => {
    await page.goto(`/reports/${reportId}`);
    await page.getByRole("button", { name: "Add report to issue" }).click();
    await page.getByRole("button", { name: "Create new" }).click();
    const issueDialog = page.getByRole("dialog", { name: "Add report to an issue" });
    const cancelHeight = await issueDialog.getByRole("button", { name: "Cancel" })
      .evaluate((button) => button.getBoundingClientRect().height);
    const createHeight = await issueDialog.getByRole("button", { name: "Create GitHub issue" })
      .evaluate((button) => button.getBoundingClientRect().height);
    expect(createHeight).toBe(cancelHeight);
    await page.getByLabel("Title", { exact: true }).fill("Renderer overlap on test page");
    await page.getByLabel("Description", { exact: true }).fill("Created by the browser test.");
    await page.getByRole("button", { name: "Create GitHub issue" }).click();

    await expect(page).toHaveURL(/\/issues\/[0-9a-f-]+$/);
    await expect(
      page.getByRole("heading", { name: "Renderer overlap on test page" }),
    ).toBeVisible();
    await expect(page.locator(`a[href="/reports/${reportId}"]`)).toBeVisible();
    await expect(page.getByRole("link", { name: "Issue #7300" }))
      .toBeVisible();
    await expect(page.getByText("GitHub", { exact: true })).toBeVisible();
    await expect(page.locator(".issue-description"))
      .toContainText("Created by the browser test.");
    const editFromReports = await page.request.post(page.url(), {
      form: {
        csrf: "browser-test-csrf",
        title: "Edited outside GitHub",
        description: "This must not be saved.",
      },
    });
    expect(editFromReports.status()).toBe(405);
    const createdIssueId = page.url().split("/").at(-1);
    const issueField = await page.request.get(
      "http://127.0.0.1:3101/test/issue-field/7300",
    );
    expect((await issueField.json()).value)
      .toBe(`http://127.0.0.1:3100/issues/${createdIssueId}`);

    const createdIssue = await page.request.get(
      "http://127.0.0.1:3101/test/latest-created-issue",
    );
    expect((await createdIssue.json()).body).toBe("Created by the browser test.");

    await page.getByRole("link", { name: "Reports", exact: true }).click();
    await page.getByLabel("Search reports").fill("state:triage");
    await page.getByLabel("Search reports").press("Enter");
    await expect(page.locator(`a[href="/reports/${reportId}"]`)).toHaveCount(0);

    await page.getByLabel("Search reports").fill("state:confirmed");
    await page.getByLabel("Search reports").press("Enter");
    await expect.poll(() => new URL(page.url()).searchParams.get("q"))
      .toBe("state:confirmed");
    await expect(page.locator(`a[href="/reports/${reportId}"]`)).toBeVisible();
  });

  test("merges tracked issues without changing GitHub state", async ({ page }) => {
    await page.goto("/issues");
    await page.getByRole("link", { name: "Renderer overlap on test page" }).click();
    await page.locator('form[action$="/merge"] select[name="destination"]').selectOption({
      label: "Intermittent navigation timeout",
    });
    await page.getByRole("button", { name: "Merge this issue" }).click();

    await expect(page.getByRole("heading", { name: "Intermittent navigation timeout" }))
      .toBeVisible();
    await expect(page.locator(`a[href="/reports/${reportId}"]`)).toBeVisible();

    const githubIssue = await page.request.get(
      "http://127.0.0.1:3101/repos/LadybirdBrowser/ladybird/issues/7300",
    );
    expect((await githubIssue.json()).state).toBe("open");
  });

  test("shows the audit log", async ({ page }) => {
    await page.goto("/operations");

    await expect(page.getByRole("heading", { name: "Audit log" })).toBeVisible();
    await expect(page.getByText("field_definitions.reorder", { exact: true })).toBeVisible();
    await expect(page.getByText("submission_source.update_state", { exact: true }))
      .toHaveCount(2);
    const stateChanges = page.locator("tbody tr").filter({ hasText: "report.update_state" });
    expect(await stateChanges.count()).toBeGreaterThanOrEqual(3);
    await expect(stateChanges.filter({ hasText: '"from":"triage","to":"confirmed"' }))
      .not.toHaveCount(0);
    await expect(stateChanges.filter({ hasText: '"from":"rejected","to":"triage"' }))
      .not.toHaveCount(0);
    await expect(stateChanges.filter({ hasText: '"to":"rejected"' }).first())
      .toBeVisible();
    await expect(page.getByText("issue.create", { exact: true })).toBeVisible();
    await expect(page.getByText("report.submitted", { exact: true }).first()).toBeVisible();
    await expect(page.getByText("session.signed_in", { exact: true }).first()).toBeVisible();
  });

  test("keeps GitHub issue state and replacement links in sync", async ({ page }) => {
    await page.goto("/issues");
    await page.getByRole("link", { name: "Intermittent navigation timeout" }).click();
    const trackedIssueId = page.url().split("/").at(-1);
    const existingIssueField = await page.request.get(
      "http://127.0.0.1:3101/test/issue-field/4812",
    );
    expect((await existingIssueField.json()).value)
      .toBe(`http://127.0.0.1:3100/issues/${trackedIssueId}`);

    const issue = {
      id: 94812,
      number: 4812,
      title: "Navigation stops after redirect",
      body: "Updated on GitHub with more reproduction details.",
      html_url: "https://github.com/LadybirdBrowser/ladybird/issues/4812",
      state: "open",
      updated_at: new Date(Date.now() + 10_000).toISOString(),
    };
    async function deliverWebhook(action: string) {
      const body = JSON.stringify({
        action,
        issue,
        repository: { full_name: "LadybirdBrowser/ladybird" },
      });
      const signature = createHmac(
        "sha256",
        "browser-test-github-webhook-secret-2026",
      ).update(body).digest("hex");
      return page.request.post("/webhooks/github", {
        data: body,
        headers: {
          "content-type": "application/json",
          "x-github-event": "issues",
          "x-hub-signature-256": `sha256=${signature}`,
        },
      });
    }

    const rejected = await page.request.post("/webhooks/github", {
      data: "{}",
      headers: { "x-github-event": "issues" },
    });
    expect(rejected.status()).toBe(403);

    expect((await deliverWebhook("edited")).status()).toBe(204);
    await page.reload();
    await expect(page.getByRole("heading", { name: issue.title })).toBeVisible();
    await expect(page.locator(".issue-description")).toContainText(issue.body);

    issue.state = "closed";
    issue.updated_at = new Date(Date.now() + 20_000).toISOString();
    expect((await deliverWebhook("closed")).status()).toBe(204);
    await page.goto("/issues?resolved=true");
    await expect(page.getByText("Resolved", { exact: true })).toBeVisible();

    issue.state = "open";
    issue.updated_at = new Date(Date.now() + 40_000).toISOString();
    expect((await deliverWebhook("reopened")).status()).toBe(204);
    await page.goto("/issues");
    await expect(page.getByText("Open", { exact: true })).toBeVisible();

    issue.state = "closed";
    expect((await deliverWebhook("deleted")).status()).toBe(204);
    await page.goto(`/issues/${trackedIssueId}`);
    await expect(page.getByText("Needs attention", { exact: true })).toBeVisible();
    const attentionBadge = page.locator(".page-header .badge-warning");
    expect(await attentionBadge.evaluate((badge) => badge.getClientRects().length)).toBe(1);
    expect(await attentionBadge.evaluate((badge) =>
      getComputedStyle(badge).whiteSpace,
    )).toBe("nowrap");

    const createDialog = page.getByRole("dialog", {
      name: "Create replacement GitHub issue?",
    });
    await expect(createDialog).toBeHidden();
    await page.getByRole("button", { name: "Create replacement GitHub issue" }).click();
    await expect(createDialog).toBeVisible();
    await createDialog.getByRole("button", { name: "Cancel" }).click();
    await expect(createDialog).toBeHidden();

    const linkDialog = page.getByRole("dialog", { name: "Link replacement issue" });
    await expect(linkDialog).toBeHidden();
    await page.getByRole("button", { name: "Link replacement issue" }).click();
    await expect(linkDialog).toBeVisible();
    await linkDialog.getByRole("textbox", { name: "GitHub issue URL" }).fill(
      "https://github.com/LadybirdBrowser/ladybird/issues/6200",
    );
    await page.request.post("http://127.0.0.1:3101/test/field-visibility", {
      data: { visibility: "all" },
    });
    await linkDialog.getByRole("button", { name: "Link issue" }).click();
    await expect(page.getByRole("link", { name: "Issue #6200" })).toBeVisible();
    await expect(page.getByText("The Reports link could not be added"))
      .toBeVisible();
    const publicField = await page.request.get(
      "http://127.0.0.1:3101/test/issue-field/6200",
    );
    expect((await publicField.json()).value).toBeNull();

    await page.request.post("http://127.0.0.1:3101/test/field-visibility", {
      data: { visibility: "organization_members_only" },
    });
    await page.reload();
    const replacementIssueField = await page.request.get(
      "http://127.0.0.1:3101/test/issue-field/6200",
    );
    expect((await replacementIssueField.json()).value)
      .toBe(`http://127.0.0.1:3100/issues/${trackedIssueId}`);
  });

  test("unlinks reports and hides an issue without changing GitHub", async ({ page }) => {
    await page.goto(`/reports/${reportId}`);
    const linkedIssue = page.locator(".report-linked-issue");
    await expect(linkedIssue.getByRole("link", { name: "Investigate renderer overlap" }))
      .toBeVisible();
    await expect(linkedIssue.getByRole("link", { name: "Issue #6200" }))
      .toBeVisible();

    await linkedIssue.getByRole("link", { name: "Investigate renderer overlap" }).click();
    const issueUrl = page.url();
    await page.getByRole("button", { name: `Unlink report ${reportId}` }).click();
    await expect(page.getByRole("link", { name: reportId })).toHaveCount(0);

    await page.goto(`/reports/${reportId}`);
    await expect(page.locator(".badge-confirmed")).toHaveText("Confirmed");
    await expect(page.locator(".report-linked-issue")).toHaveCount(0);

    await page.goto(issueUrl);
    const remainingReport = await page.locator('a[href^="/reports/"]').first()
      .getAttribute("href");
    expect(remainingReport).not.toBeNull();
    await page.getByRole("button", { name: "Delete issue" }).click();
    const confirmation = page.getByRole("dialog", { name: "Delete this issue?" });
    await expect(confirmation).toBeVisible();
    await confirmation.getByRole("button", { name: "Cancel" }).click();
    await expect(confirmation).toBeHidden();
    await page.getByRole("button", { name: "Delete issue" }).click();
    await confirmation.getByRole("button", { name: "Delete" }).click();
    await expect(page).toHaveURL(/\/issues$/);
    expect((await page.request.get(issueUrl)).status()).toBe(404);

    await page.goto(remainingReport!);
    await expect(page.locator(".report-linked-issue")).toHaveCount(0);
    const githubIssue = await page.request.get(
      "http://127.0.0.1:3101/repos/LadybirdBrowser/ladybird/issues/6200",
    );
    expect((await githubIssue.json()).state).toBe("open");
  });
});
