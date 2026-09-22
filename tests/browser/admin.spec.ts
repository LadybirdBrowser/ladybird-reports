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
    /\/assets\/[0-9a-f]{64}\/ladybird-mark\.png/,
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

test("refreshes GitHub tokens and keeps an active session for 24 hours", async ({ page }) => {
  const fakeGithub = "http://127.0.0.1:3101";
  const before = await (await page.request.get(`${fakeGithub}/test/token-refresh-count`)).json();

  await page.goto("/login");
  await page.getByRole("link", { name: "Continue with GitHub" }).click();
  await expect(page).toHaveURL(/127\.0\.0\.1:3100\/$/);

  const after = await (await page.request.get(`${fakeGithub}/test/token-refresh-count`)).json();
  expect(after.count).toBeGreaterThan(before.count);

  const sessionCookie = (await page.context().cookies()).find((cookie) => cookie.name === "session");
  expect(sessionCookie).toBeDefined();
  expect(sessionCookie!.expires - Date.now() / 1000).toBeGreaterThan(23 * 60 * 60);

  await page.reload();
  await expect(page).toHaveURL(/127\.0\.0\.1:3100\/$/);
  const rotatedAgain = await (await page.request.get(`${fakeGithub}/test/token-refresh-count`)).json();
  expect(rotatedAgain.count).toBeGreaterThan(after.count);

  await page.getByRole("button", { name: "Open account menu for browser-tester" }).click();
  await page.getByRole("button", { name: "Sign out" }).click();
  await expect(page).toHaveURL(/\/login$/);
  expect((await page.context().cookies()).some((cookie) => cookie.name === "session")).toBe(false);
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

  test("issue pages do not wait for GitHub to respond", async ({ page }) => {
    await page.goto("/issues");
    const issuePath = await page.getByRole("link", {
      name: "Intermittent navigation timeout",
    }).getAttribute("href");

    const fakeGithub = "http://127.0.0.1:3101/test/issue-response-delay";
    await page.request.post(`${fakeGithub}?milliseconds=1000`);
    try {
      const started = Date.now();
      const response = await page.request.get(issuePath!);
      expect(response.status()).toBe(200);
      expect(Date.now() - started).toBeLessThan(700);
    } finally {
      await page.request.post(`${fakeGithub}?milliseconds=0`);
    }
  });

  test("expired sessions do not download the login page as an attachment", async ({ page }) => {
    await page.goto(`/reports/${reportId}`);
    const attachment = page.getByRole("link", { name: "Download crash-diagnostics.txt" });
    await expect(attachment).not.toHaveAttribute("download");
    await page.context().clearCookies();

    let downloaded = false;
    page.on("download", () => { downloaded = true; });
    await attachment.click();
    await expect(page).toHaveURL(/\/login\?next=%2Fattachments%2F/);
    expect(downloaded).toBe(false);
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
    await expect(page.getByText(
      "Verification failed: false at Libraries/LibMedia/FFmpeg/FFmpegVideoDecoder.cpp:235",
    )).toBeVisible();
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
    await expect(page.locator(".badge-triage")).toHaveText("Needs triage");
    await page.getByLabel("Show raw").check();
    await expect(page.locator(".stack-raw-text"))
      .toContainText("Native stack (binary build ID, object address):");
    await expect(page.locator(".stack-frame-table")).toBeHidden();
    await page.getByLabel("Show raw").uncheck();
    await expect(page.locator(".stack-frame-table")).toBeVisible();
    await expect(page.getByText("macOS", { exact: true }).first()).toBeVisible();
    await expect(page.getByText("arm64", { exact: true }).first()).toBeVisible();
    await expect(page.locator(".definition-list > div")).toHaveCount(5);
    await expect(page.getByRole("heading", { name: "Additional fields" })).toBeVisible();
    await expect(page.locator(".report-field-value").filter({ hasText: "<script>" })).toBeVisible();
    await expect(page.getByRole("link", { name: "Filter reports by Stack trace" })).toHaveCount(0);
    await expect(page.getByRole("heading", { name: "Attachments" }).locator(".."))
      .toContainText("1 file");
    const attachmentRow = page.locator(".attachment-table tbody tr").first();
    await expect(attachmentRow).toContainText("crash-diagnostics.txt");
    await expect(attachmentRow).toContainText("text/plain");
    await expect(attachmentRow).toContainText("3.7 KiB");
    await expect(attachmentRow.locator("td:nth-child(-n+3) a")).toHaveCount(0);
    const viewLink = attachmentRow.getByRole("link", { name: "View crash-diagnostics.txt" });
    const downloadLink = attachmentRow.getByRole("link", { name: "Download crash-diagnostics.txt" });
    const viewResponse = await page.request.get(await viewLink.getAttribute("href")!);
    expect(viewResponse.headers()["content-disposition"])
      .toContain('inline; filename="crash-diagnostics.txt"');
    const attachmentResponse = await page.request.get(await downloadLink.getAttribute("href")!);
    expect(attachmentResponse.headers()["content-disposition"])
      .toContain('attachment; filename="crash-diagnostics.txt"');
    const download = page.waitForEvent("download");
    await downloadLink.click();
    expect((await download).suggestedFilename()).toBe("crash-diagnostics.txt");
    await viewLink.click();
    await expect(page).toHaveURL(/\/attachments\/.*\?inline=true/);
    await page.goBack();
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
    expect(new URL(page.url()).searchParams.get("q")).toBe("platform:macos");
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
    await expect(fields).toHaveCount(6);
    await expect(fields.nth(4).locator("dt")).toHaveText("Submitted");
    await expect(fields.nth(5).locator("dt")).toHaveText("Build");

    const submitted = await fields.nth(4).boundingBox();
    const build = await fields.nth(5).boundingBox();
    expect(submitted).not.toBeNull();
    expect(build).not.toBeNull();
    expect(build!.width).toBeGreaterThanOrEqual(submitted!.width - 2);
    expect(await fields.nth(4).evaluate((field) =>
      getComputedStyle(field).borderBottomWidth,
    )).toBe("1px");

    await page.setViewportSize({ width: 390, height: 800 });
    expect(await page.evaluate(() => document.documentElement.scrollWidth))
      .toBeLessThanOrEqual(390);
  });

  test("sets security headers and caches versioned assets", async ({ page, request }) => {
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

    const stylesheetUrl = await page.locator('link[rel="stylesheet"]').getAttribute("href");
    expect(stylesheetUrl).toMatch(/^\/assets\/[0-9a-f]{64}\/application\.css$/);
    const assetBase = stylesheetUrl!.slice(0, -"application.css".length);
    await expect(page.locator(".brand-mark img"))
      .toHaveAttribute("src", `${assetBase}ladybird-mark.png`);

    for (const asset of ["application.css", "application.js", "reports.js", "list-search.js", "github-icon.svg"]) {
      const firstResponse = await request.get(`${assetBase}${asset}`);
      const etag = firstResponse.headers()["etag"];

      expect(firstResponse.status()).toBe(200);
      expect(firstResponse.headers()["cache-control"]).toBe(
        "public, max-age=31536000, immutable",
      );
      expect(etag).toMatch(/^\"[0-9a-f]{64}\"$/);
    }

    const markResponse = await request.get(`${assetBase}ladybird-mark.png`);

    expect(markResponse.status()).toBe(200);
    expect(markResponse.headers()["content-type"]).toBe("image/png");
    expect(markResponse.headers()["cache-control"]).toBe(
      "public, max-age=31536000, immutable",
    );
    expect(markResponse.headers()["etag"]).toMatch(/^\"[0-9a-f]{64}\"$/);

    const unknownVersion = await request.get("/assets/invalid/application.css");
    expect(unknownVersion.status()).toBe(404);

    const legacyAsset = await request.get("/assets/application.css");
    expect(legacyAsset.headers()["cache-control"])
      .toBe("public, max-age=0, must-revalidate");
    const revalidatedLegacyAsset = await request.get("/assets/application.css", {
      headers: { "if-none-match": legacyAsset.headers()["etag"] },
    });

    expect(revalidatedLegacyAsset.status()).toBe(304);
  });

  test("reuses cached assets when navigating between management pages", async ({ page }) => {
    await page.goto("/issues");
    await page.goto("/settings");
    await page.goto("/issues");

    const assetTransfers = await page.evaluate(() =>
      performance.getEntriesByType("resource")
        .filter((entry) => new URL(entry.name).pathname.startsWith("/assets/"))
        .map((entry) => (entry as PerformanceResourceTiming).transferSize),
    );
    expect(assetTransfers.length).toBeGreaterThan(0);
    expect(assetTransfers).toEqual(assetTransfers.map(() => 0));
  });

  test("searches tracked and GitHub issues in one selector", async ({ page }) => {
    await page.goto("/issues");
    await expect(page.getByText("Intermittent navigation timeout")).toBeVisible();

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
    await expect(issueDialog.locator('input[data-entity-value]'))
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

  test("filters issues with the shared query interaction", async ({ page }) => {
    await page.goto("/issues");
    const search = page.getByLabel("Search issues");
    await expect(search).toHaveValue("state:unresolved state:needs_attention");
    await page.evaluate(() => ((window as any).issuePageStayedLoaded = true));

    await search.fill("state:resolved");
    await page.waitForTimeout(800);
    await expect(search).toHaveValue("state:resolved");
    expect(new URL(page.url()).searchParams.has("q")).toBe(false);
    await search.press("Enter");

    await expect.poll(() => new URL(page.url()).searchParams.get("q"))
      .toBe("state:resolved");
    expect(await page.evaluate(() => (window as any).issuePageStayedLoaded)).toBe(true);
    await expect(page.getByText("No matching issues")).toBeVisible();

    await search.fill("state:unresolved github:4812");
    await search.press("Enter");
    await expect(page.getByRole("link", { name: "Intermittent navigation timeout" }))
      .toBeVisible();
    await expect(page.locator("[data-list-results] tbody tr")).toHaveCount(1);

    await search.fill("state:unresolved state:needs_attention");
    await search.press("Enter");
    await expect.poll(() => new URL(page.url()).searchParams.get("q"))
      .toBe("state:unresolved state:needs_attention");

    await search.fill("sta");
    await expect(page.getByRole("option", { name: /^state:/ })).toBeVisible();
    await search.press("ArrowDown");
    await search.press("Enter");
    await expect(search).toHaveValue("state:");
    await expect(page.getByRole("option", { name: /unresolved/ })).toBeVisible();
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
    await expect(confirmation.getByRole("checkbox", {
      name: "Reject all triage reports from this IP",
    })).toBeVisible();
    await confirmation.getByRole("button", { name: "Cancel" }).click();
    await expect(confirmation).toBeHidden();

    await page.getByRole("button", { name: "Block submission IP" }).click();
    await confirmation.getByRole("checkbox").uncheck();
    await confirmation.getByRole("button", { name: "Block IP" }).click();
    await expect(page.getByText("IP is blocked indefinitely")).toBeVisible();
    await expect(page.locator(".badge-triage")).toHaveText("Needs triage");

    await page.getByRole("button", { name: "Unblock submission IP" }).click();
    await expect(page.getByRole("button", { name: "Block submission IP" })).toBeVisible();
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
    await expect(orderedRows.nth(0)).toContainText("failure_reason");
    await expect(orderedRows.nth(1)).toContainText("signal");
    await expect(orderedRows.nth(2)).toContainText("stack");

    await expect(page.getByText("Stack trace is now at position 3.")).toBeVisible();
    await page.reload();
    await expect(orderedRows.nth(0)).toContainText("failure_reason");
    await expect(orderedRows.nth(1)).toContainText("signal");
    await expect(orderedRows.nth(2)).toContainText("stack");
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
    await expect(page.locator(".page-header h1")).toHaveCSS("text-decoration-line", "line-through");
    await page.getByRole("button", { name: "Restore report" }).click();
    await expect(page.locator(".badge-triage")).toHaveText("Needs triage");
    await expect(page.locator(".page-header h1")).toHaveCSS("text-decoration-line", "none");
    await page.getByRole("button", { name: "Reject report" }).first().click();
    await confirmation.getByRole("button", { name: "Reject report" }).click();
    await expect(page.locator(".badge-rejected")).toHaveText("Rejected");
    await expect(page.locator(".page-header h1")).toHaveCSS("text-decoration-line", "line-through");
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

  test("links an exact issue match and unlinks it from the report", async ({ page }) => {
    await page.goto("/issues");
    await page.getByRole("link", { name: "Intermittent navigation timeout" }).click();

    const matches = page.getByRole("region", { name: "Potential matches" });
    await expect(matches).toBeVisible();
    const candidate = matches.getByRole("link", { name: /WebContent::/ }).first();
    const candidateUrl = await candidate.getAttribute("href");
    expect(candidateUrl).toMatch(/^\/reports\/[0-9a-f-]+$/);

    await matches.getByRole("button", { name: /Link report/ }).first().click();
    await expect(page).toHaveURL(/\/issues\/[0-9a-f-]+$/);
    await expect(page.locator(`a[href="${candidateUrl}"]`)).toBeVisible();

    await page.goto(candidateUrl!);
    await expect(page.locator(".badge-confirmed")).toHaveText("Confirmed");
    await expect(page.locator(".report-linked-issue")).toBeVisible();
    await page.getByRole("button", { name: "Unlink report", exact: true }).click();
    await expect(page).toHaveURL(candidateUrl!);
    await expect(page.locator(".badge-triage")).toHaveText("Needs triage");
    await expect(page.getByRole("button", { name: "Add report to issue" })).toBeVisible();
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
      body: "## Reproduction\n\n- [x] Open the page\n\n| Result | Status |\n| --- | --- |\n| Navigation | Stalled |\n\n```text\nstack frame\n```",
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
    const description = page.locator(".issue-description");
    await expect(description.getByRole("heading", { name: "Reproduction" })).toBeVisible();
    await expect(description.getByRole("checkbox")).toBeChecked();
    await expect(description.locator("table td").first()).toHaveText("Navigation");
    await expect(description.locator("pre code")).toContainText("stack frame");

    issue.state = "closed";
    issue.updated_at = new Date(Date.now() + 20_000).toISOString();
    expect((await deliverWebhook("closed")).status()).toBe(204);
    await page.goto("/issues?q=state%3Aresolved");
    await expect(page.getByText("Resolved", { exact: true })).toBeVisible();

    issue.state = "open";
    issue.updated_at = new Date(Date.now() + 40_000).toISOString();
    expect((await deliverWebhook("reopened")).status()).toBe(204);
    await page.goto("/issues");
    await expect(page.locator(`tr:has(a[href="/issues/${trackedIssueId}"])`)
      .getByText("Unresolved", { exact: true })).toBeVisible();

    issue.state = "closed";
    expect((await deliverWebhook("deleted")).status()).toBe(204);
    await page.goto(`/issues/${trackedIssueId}`);
    await expect(page.getByText("Needs attention", { exact: true })).toBeVisible();
    const matches = page.getByRole("region", { name: "Potential matches" });
    const candidate = matches.getByRole("link", { name: /WebContent::/ }).first();
    const candidateUrl = await candidate.getAttribute("href");
    expect(candidateUrl).toMatch(/^\/reports\//);
    await expect(matches.getByRole("button", { name: /Link report/ }).first())
      .toBeVisible();

    await page.goto(candidateUrl!);
    await page.getByRole("button", { name: "Add report to issue" }).click();
    const issueDialog = page.getByRole("dialog", { name: "Add report to an issue" });
    const suggestion = issueDialog.locator(".issue-suggestion").filter({
      hasText: "Navigation stops after redirect",
    });
    await expect(suggestion).toBeVisible();
    await expect(suggestion).toContainText("Navigation stops after redirect");
    await expect(suggestion).toContainText("GitHub link needs attention");
    await expect(suggestion.getByRole("button", { name: "Add to issue" }))
      .toBeVisible();

    await issueDialog.getByRole("combobox", { name: "Search issues" })
      .fill("Navigation stops after redirect");
    await expect(page.getByRole("option", { name: /Navigation stops after redirect/ }))
      .toBeVisible();
    await issueDialog.getByRole("combobox", { name: "Search issues" }).press("Escape");
    await suggestion.getByRole("button", { name: "Add to issue" }).click();
    await expect(page).toHaveURL(`/issues/${trackedIssueId}`);
    await page.goto(candidateUrl!);
    await expect(page.locator(".report-linked-issue")).toBeVisible();
    await page.getByRole("button", { name: "Unlink report", exact: true }).click();
    await expect(page.locator(".badge-triage")).toHaveText("Needs triage");

    await page.goto(`/issues/${trackedIssueId}`);
    await matches.getByRole("button", { name: /Link report/ }).first().click();
    await expect(page).toHaveURL(`/issues/${trackedIssueId}`);
    await page.goto(candidateUrl!);
    await expect(page.locator(".report-linked-issue")).toBeVisible();
    await page.getByRole("button", { name: "Unlink report", exact: true }).click();
    await expect(page.locator(".badge-triage")).toHaveText("Needs triage");

    await page.goto(`/issues/${trackedIssueId}`);
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

  test("unlinks reports and rejects an issue without changing GitHub", async ({ page }) => {
    await page.goto(`/reports/${reportId}`);
    const linkedIssue = page.locator(".report-linked-issue");
    const issueLink = linkedIssue.locator('a[href^="/issues/"]');
    await expect(issueLink).toBeVisible();
    await expect(linkedIssue.getByRole("link", { name: "Issue #7300" }))
      .toBeVisible();

    await issueLink.click();
    const issueUrl = page.url();
    const assignedRow = page.locator(".clickable-report-row").filter({
      has: page.locator(`a[href="/reports/${reportId}"]`),
    }).first();
    const versionCell = await assignedRow.locator("td").nth(1).boundingBox();
    expect(versionCell).not.toBeNull();
    await page.mouse.click(versionCell!.x + versionCell!.width / 2, versionCell!.y + versionCell!.height / 2);
    await expect(page).toHaveURL(`/reports/${reportId}`);
    await page.goto(issueUrl);
    await page.getByRole("button", { name: `Unlink report ${reportId}` }).click();

    await page.goto(`/reports/${reportId}`);
    await expect(page.locator(".badge-triage")).toHaveText("Needs triage");
    await expect(page.locator(".report-linked-issue")).toHaveCount(0);

    await page.goto(issueUrl);
    await page.getByRole("button", { name: "Reject issue" }).first().click();
    const confirmation = page.getByRole("dialog", { name: "Reject this issue?" });
    await expect(confirmation).toBeVisible();
    await confirmation.getByRole("button", { name: "Cancel" }).click();
    await expect(confirmation).toBeHidden();
    await page.getByRole("button", { name: "Reject issue" }).first().click();
    await confirmation.getByRole("button", { name: "Reject issue" }).click();
    await expect(page).toHaveURL(issueUrl);
    await expect(page.locator(".page-header .badge-rejected")).toHaveText("Rejected");

    await page.goto("/issues?q=state%3Arejected");
    await expect(page.locator(`a[href="${new URL(issueUrl).pathname}"]`)).toBeVisible();

    await page.goto(`/reports/${reportId}`);
    await expect(page.locator(".report-linked-issue")).toHaveCount(0);
    const githubIssue = await page.request.get(
      "http://127.0.0.1:3101/repos/LadybirdBrowser/ladybird/issues/7300",
    );
    expect((await githubIssue.json()).state).toBe("open");
  });



});
