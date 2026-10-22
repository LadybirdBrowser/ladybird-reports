import { type Locator, type Page, expect, test } from "@playwright/test";
import { createHmac } from "node:crypto";

// The addresses of the test server and the fake GitHub, as set in playwright.config.ts.
const adminUrl = "http://127.0.0.1:3100";
const fakeGithub = "http://127.0.0.1:3101";
const reportId = "01a0a536-01cd-7ac7-a3cf-ae6a2d5030e5";

// Value buttons only show while their value is hovered, so move the mouse onto the value first.
async function hoverValueOf(button: Locator) {
  await button.locator(
    "xpath=ancestor::*[contains(@class,'value-area')][1]",
  ).hover();
}

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

// Requests accept a session from what is stored with it and never call GitHub.
test("keeps an active session for 24 hours without calling GitHub", async ({ page }) => {
  const refreshCount = async () =>
    (await (await page.request.get(`${fakeGithub}/test/token-refresh-count`)).json()).count;

  await page.goto("/login");
  await page.getByRole("link", { name: "Continue with GitHub" }).click();
  await expect(page).toHaveURL(`${adminUrl}/`);

  const sessionCookie = (await page.context().cookies()).find((cookie) => cookie.name === "session");
  expect(sessionCookie).toBeDefined();
  expect(sessionCookie!.expires - Date.now() / 1000).toBeGreaterThan(23 * 60 * 60);

  const afterSignIn = await refreshCount();
  await page.reload();
  await expect(page).toHaveURL(`${adminUrl}/`);
  expect(await refreshCount()).toBe(afterSignIn);

  await page.getByRole("button", { name: "Open account menu for browser-tester" }).click();
  await page.getByRole("button", { name: "Sign out" }).click();
  await expect(page).toHaveURL(/\/login$/);
  expect((await page.context().cookies()).some((cookie) => cookie.name === "session")).toBe(false);
});

test("records a denied sign-in once and still lets maintainers in", async ({ page }) => {
  const setDenied = (denied: boolean) =>
    page.request.post(`${fakeGithub}/test/membership-denied?denied=${denied}`);

  await setDenied(true);
  try {
    for (let attempt = 0; attempt < 2; attempt += 1) {
      await page.goto("/login");
      await page.getByRole("link", { name: "Continue with GitHub" }).click();
      await expect(page.getByText("Access denied")).toBeVisible();
      expect((await page.context().cookies()).some((cookie) => cookie.name === "session"))
        .toBe(false);
    }
  } finally {
    await setDenied(false);
  }

  await page.goto("/login");
  await page.getByRole("link", { name: "Continue with GitHub" }).click();
  await expect(page).toHaveURL(`${adminUrl}/`);

  await page.goto("/operations");
  const denied = page.locator("[data-rows] tr").filter({ hasText: "session.denied" });
  await expect(denied).toHaveCount(1);
  await expect(denied.locator("td").nth(2)).toHaveText("browser-tester");
  await expect(denied.locator("code")).toContainText('"github_id":12345');
  await expect(denied.locator("code")).toContainText('"login":"browser-tester"');
});

test("sign-in ignores an external return destination", async ({ page }) => {
  await page.goto("/login?next=%2F%2Fevil.example%2Fpath");
  await expect(page.getByRole("link", { name: "Continue with GitHub" }))
    .toHaveAttribute("href", "/auth/github");
});

// Reports that share a stack signature are folded into groups in the report list. Most tests want to see every report,
// so they open the groups first.
async function openGroups(page: Page) {
  const closed = page.locator('.report-group-toggle[aria-expanded="false"]');
  while ((await closed.count()) > 0) {
    await closed.first().click();
  }
}

test.describe("authenticated management UI", () => {
  test.use({
    storageState: "target/browser-test-storage-state.json",
  });

  test("issue pages do not wait for GitHub to respond", async ({ page }) => {
    await page.goto("/issues");
    const issuePath = await page.getByRole("link", {
      name: "Intermittent navigation timeout",
    }).getAttribute("href");

    const delayUrl = `${fakeGithub}/test/issue-response-delay`;
    await page.request.post(`${delayUrl}?milliseconds=1000`);
    try {
      const started = Date.now();
      const response = await page.request.get(issuePath!);
      expect(response.status()).toBe(200);
      expect(Date.now() - started).toBeLessThan(700);
    } finally {
      await page.request.post(`${delayUrl}?milliseconds=0`);
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

  test.describe("status quick filters", () => {
    const defaultQuery = "state:triage state:confirmed";

    async function expectFilters(page: Page, query: string, active: string[]) {
      await expect(page.locator('input[name="q"]')).toHaveValue(query);
      for (const name of ["Needs triage", "Confirmed", "Resolved", "Rejected"]) {
        const pill = page.getByRole("navigation", { name: "Filter by report status" }).getByRole("link", { name });
        if (active.includes(name)) {
          await expect(pill).toHaveAttribute("aria-current", "true");
        } else {
          await expect(pill).not.toHaveAttribute("aria-current", "true");
        }
      }
    }

    const pill = (page: Page, name: string) =>
      page.getByRole("navigation", { name: "Filter by report status" }).getByRole("link", { name });

    test("show the filters of the query as active", async ({ page }) => {
      await page.goto("/");
      await expectFilters(page, defaultQuery, ["Needs triage", "Confirmed"]);

      await page.goto("/?q=kind:crash%20state:Resolved");
      await expectFilters(page, "kind:crash state:Resolved", ["Resolved"]);
    });

    test("a click replaces the query, and another click on the only active pill restores the default", async ({ page }) => {
      await page.goto("/?q=kind:crash%20state:triage");

      await pill(page, "Resolved").click();
      await expectFilters(page, "state:resolved", ["Resolved"]);
      await expect(page).toHaveURL(/q=state%3Aresolved$/);

      await pill(page, "Resolved").click();
      await expectFilters(page, defaultQuery, ["Needs triage", "Confirmed"]);
    });

    test("a click on one of several active pills keeps only that one and the rest of the query", async ({ page }) => {
      await page.goto("/");
      await pill(page, "Confirmed").click();
      await expectFilters(page, "state:confirmed", ["Confirmed"]);

      await page.goto("/?q=kind:crash%20state:triage%20state:rejected");
      await expectFilters(page, "kind:crash state:triage state:rejected", ["Needs triage", "Rejected"]);
      await pill(page, "Rejected").click();
      await expectFilters(page, "kind:crash state:rejected", ["Rejected"]);

      await pill(page, "Rejected").click();
      await expectFilters(page, "kind:crash state:triage state:confirmed", ["Needs triage", "Confirmed"]);
    });

    test("Cmd or Ctrl adds a filter, or removes it when it is active", async ({ page }) => {
      await page.goto("/?q=kind:crash%20state:resolved");

      await pill(page, "Confirmed").click({ modifiers: ["ControlOrMeta"] });
      await expectFilters(page, "kind:crash state:resolved state:confirmed", ["Resolved", "Confirmed"]);

      await pill(page, "Resolved").click({ modifiers: ["ControlOrMeta"] });
      await expectFilters(page, "kind:crash state:confirmed", ["Confirmed"]);

      await pill(page, "Confirmed").click({ modifiers: ["ControlOrMeta"] });
      await expectFilters(page, "kind:crash", []);
    });

    test("follow what is typed in the search box", async ({ page }) => {
      await page.goto("/");
      await page.locator('input[name="q"]').fill("state:rejected");
      await expectFilters(page, "state:rejected", ["Rejected"]);
    });
  });

  test("filters the issue list with status pills", async ({ page }) => {
    const bar = page.getByRole("navigation", { name: "Filter by issue status" });
    // The name of a pill also holds its count, and "Resolved" is part of "Unresolved".
    const pill = (name: string) => bar.getByRole("link", { name: new RegExp(`^${name}\\b`) });

    await page.goto("/issues");
    await expect(page.locator('input[name="q"]')).toHaveValue("state:unresolved state:needs_attention");
    await expect(pill("Unresolved")).toHaveAttribute("aria-current", "true");
    await expect(pill("Needs attention")).toHaveAttribute("aria-current", "true");
    await expect(pill("Resolved")).not.toHaveAttribute("aria-current", "true");
    // Every pill shows how many issues are in its state.
    for (const name of ["Unresolved", "Needs attention", "Resolved", "Rejected"]) {
      await expect(pill(name).locator(".filter-pill-count")).toHaveText(/^\d+$/);
    }

    // The same rules as the report list: a click replaces the query and updates the list, and a second click on
    // the only active pill returns to the default.
    await pill("Resolved").click();
    await expect(page.locator('input[name="q"]')).toHaveValue("state:resolved");
    await expect(pill("Resolved")).toHaveAttribute("aria-current", "true");
    await expect(page).toHaveURL(/q=state%3Aresolved$/);
    await pill("Resolved").click();
    await expect(page.locator('input[name="q"]'))
      .toHaveValue("state:unresolved state:needs_attention");
  });

  test("serves text attachments uncompressed, and compressed in transit when asked", async ({ page }) => {
    await page.goto(`/reports/${reportId}`);
    const diagnostics = page.getByRole("link", { name: "Download crash-diagnostics.txt" });
    const href = (await diagnostics.getAttribute("href"))!;
    const screenshotHref = (await page.locator(".overview-screenshot a").getAttribute("href"))!;

    // Stored compressed or not, the contents are what was uploaded, whatever the transport does with them.
    for (const encoding of ["identity", "gzip"]) {
      const response = await page.request.get(href, { headers: { "accept-encoding": encoding } });
      expect(response.headers()["content-type"]).toBe("text/plain; charset=utf-8");
      expect(response.headers()["content-encoding"]).toBe(encoding === "gzip" ? "gzip" : undefined);
      const text = await response.text();
      expect(text).toContain("Ladybird crash report, format 1");
      expect(text).toContain("Verification failed: false at Libraries/LibMedia/FFmpeg/FFmpegVideoDecoder.cpp:235");
      expect(Buffer.byteLength(text)).toBe(3756);
    }

    // An image is already compressed, so it is left as it is.
    const image = await page.request.get(screenshotHref, { headers: { "accept-encoding": "gzip" } });
    expect(image.headers()["content-type"]).toBe("image/png");
    expect(image.headers()["content-encoding"]).toBeUndefined();
  });

  test("shows the screenshot of a report with the overview, within a bounded size", async ({ page }) => {
    await page.goto(`/reports/${reportId}`);

    const screenshot = page.locator(".overview-screenshot img");
    await expect(screenshot).toBeVisible();
    await expect.poll(() => screenshot.evaluate((image: HTMLImageElement) => image.naturalWidth)).toBe(1200);

    // The fixture's screenshot is 1200 by 3000 pixels; it shrinks to fit in 24rem and keeps its proportions.
    const shown = (await screenshot.boundingBox())!;
    expect(shown.height).toBeLessThanOrEqual(24 * 16 + 4);
    expect(shown.width / shown.height).toBeCloseTo(1200 / 3000, 1);

    // Centered in the width of the overview.
    const card = (await page.locator(".overview-screenshot").boundingBox())!;
    expect(Math.abs(shown.x + shown.width / 2 - (card.x + card.width / 2))).toBeLessThan(2);

    const link = page.locator(".overview-screenshot a");
    await expect(link).toHaveAttribute("href", /^\/attachments\/[0-9a-f-]+\?inline=true$/);
    const full = await page.request.get((await link.getAttribute("href"))!);
    expect(full.headers()["content-type"]).toBe("image/png");
  });

  test("shows setup credentials and escapes unknown report fields", async ({ page }) => {
    const response = await page.goto("/");

    expect(response?.headers()["x-request-id"]).toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
    );
    await expect(page.getByText("Reporting database setup is incomplete.")).toBeVisible();
    await openGroups(page);
    const reportLink = page.locator(`a[href="/reports/${reportId}"]`);
    await expect(reportLink).toHaveText(
      "Web compatibility: https://example.test/checkout",
    );
    await expect(page.locator("tbody")).not.toContainText(reportId);
    const received = reportLink.locator("xpath=ancestor::tr").locator("time");
    await expect(received).toHaveText("now");
    await expect(received).toHaveAttribute(
      "title",
      /\d{2} [A-Z][a-z]{2} \d{4}, \d{2}:\d{2} UTC/,
    );

    await reportLink.click();
    await expect(page.getByRole("heading", {
      name: "Web compatibility: https://example.test/checkout",
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
    await expect(page.getByText("macOS · arm64", { exact: true }).first()).toBeVisible();
    // Type, version, page URL, platform and architecture, submission time, and the screenshot.
    await expect(page.locator(".definition-list > div")).toHaveCount(6);
    await expect(page.getByRole("heading", { name: "Additional fields" })).toBeVisible();
    await expect(page.locator(".report-field-value").filter({ hasText: "<script>" })).toBeVisible();
    await expect(page.getByRole("link", { name: "Filter reports by Stack trace" })).toHaveCount(0);
    await expect(page.getByRole("heading", { name: "Attachments" }).locator(".."))
      .toContainText("2 files");
    // In alphabetical order, with the sizes lined up on their right edges.
    await expect(page.locator(".attachment-table .attachment-name")).toHaveText([
      "crash-diagnostics.txt",
      "screenshot.png",
    ]);
    const sizeEdges = await page.locator(".attachment-table td.attachment-size").evaluateAll(
      (cells) => cells.map((cell) => Math.round(cell.getBoundingClientRect().right)),
    );
    expect(sizeEdges).toHaveLength(2);
    expect(sizeEdges[0]).toBe(sizeEdges[1]);
    await expect(page.locator(".attachment-table td.attachment-size").first()).toHaveCSS("text-align", "right");
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

    const platformFilter = page.getByRole("region", { name: "Overview" })
      .getByRole("link", { name: "Filter reports by Platform & Architecture" });
    await hoverValueOf(platformFilter);
    await platformFilter.click();
    expect(new URL(page.url()).searchParams.get("q")).toBe("platform:macos architecture:arm64");
    await expect(page.locator(`a[href="/reports/${reportId}"]`)).toBeVisible();

    expect(await page.evaluate(() => (window as any).fixtureWasExecuted)).toBeUndefined();
  });

  test("shows the page URL with the key facts instead of in the diagnostics", async ({ page }) => {
    const pageUrl = "https://example.test/checkout?step=payment";
    await page.goto(`/reports/${reportId}`);

    const overview = page.getByRole("region", { name: "Overview" });
    await expect(overview.locator("dt")).toHaveText([
      "Report type",
      "Browser version",
      "Page URL",
      "Platform & Architecture",
      "Submitted",
      "Screenshot",
    ]);
    await expect(overview.locator(".definition-list > div").filter({ hasText: "Page URL" }))
      .toContainText(pageUrl);

    // Every field is separated from its neighbours by a line, whatever the row above it looks like.
    const box = async (label: string) => (await overview.locator(".definition-list > div")
      .filter({ hasText: label }).first().boundingBox())!;
    const [platform, submitted, screenshot] =
      [await box("Platform & Architecture"), await box("Submitted"), await box("Screenshot")];
    expect(submitted.x - (platform.x + platform.width)).toBeCloseTo(1, 0);
    expect(screenshot.y - (submitted.y + submitted.height)).toBeCloseTo(1, 0);
    expect(screenshot.y - (platform.y + platform.height)).toBeCloseTo(1, 0);

    // Without the screenshot, as for a report that has none, the last two fields still share a row.
    await overview.locator(".overview-screenshot").evaluate((element) => element.closest("div")!.remove());
    const [lastPlatform, lastSubmitted] = [await box("Platform & Architecture"), await box("Submitted")];
    expect(lastSubmitted.y).toBeCloseTo(lastPlatform.y, 0);
    expect(lastSubmitted.width).toBeCloseTo(lastPlatform.width, 0);

    // It is never a plain link to a page an anonymous user chose, and it appears once.
    await expect(overview.getByRole("link", { name: /example\.test/ })).toHaveCount(0);
    await expect(page.locator(".report-layout").getByText(pageUrl)).toHaveCount(1);

    const urlFilter = overview.getByRole("link", { name: "Filter reports by Page URL" });
    await hoverValueOf(urlFilter);
    await urlFilter.click();
    expect(new URL(page.url()).searchParams.get("q")).toContain("url:");
    await expect(page.locator(`a[href="/reports/${reportId}"]`)).toBeVisible();
  });

  test("warns before opening a page URL from a report", async ({ page, context }) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"]);
    const pageUrl = "https://example.test/checkout?step=payment";
    await page.goto(`/reports/${reportId}`);

    const dialog = page.getByRole("dialog", { name: "Open this address?" });
    await expect(dialog).toBeHidden();
    await page.getByRole("region", { name: "Overview" })
      .getByRole("button", { name: pageUrl }).click();
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText("has not been checked");
    await expect(dialog.getByLabel("Address")).toHaveValue(pageUrl);

    // Focus starts on the safe choice, not on the action the warning is about.
    await expect(dialog.getByRole("button", { name: "Close", exact: true })).toBeFocused();

    const open = dialog.getByRole("link", { name: "Open in new tab" });
    await expect(open).toHaveAttribute("href", pageUrl);
    await expect(open).toHaveAttribute("target", "_blank");
    await expect(open).toHaveAttribute("rel", /noopener/);

    await dialog.getByRole("button", { name: "Copy URL" }).click();
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(pageUrl);

    await dialog.getByRole("button", { name: "Close", exact: true }).click();
    await expect(dialog).toBeHidden();
  });

  test("links a commit ID to the commit in the upstream repository", async ({ page }) => {
    await page.goto(`/reports/${reportId}`);

    await expect(page.getByRole("link", { name: "654cf9b187384fa8855eac4fbafaa70e75497083" }))
      .toHaveAttribute(
        "href",
        /^https:\/\/github\.com\/[\w.-]+\/[\w.-]+\/commit\/654cf9b187384fa8855eac4fbafaa70e75497083$/,
      );
    await expect(page.getByRole("link", { name: "654cf9b187384fa8855eac4fbafaa70e75497083" }))
      .not.toHaveAttribute("target", /.+/);
  });

  test("copies field values from button groups that sit before the filter button", async ({ page, context }) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"]);
    await page.goto(`/reports/${reportId}`);
    const clipboard = () => page.evaluate(() => navigator.clipboard.readText());

    // In a group, the copy button comes first and the filter button follows.
    const overview = page.getByRole("region", { name: "Overview" });
    const platform = overview.locator(".definition-list > div").filter({ hasText: "Platform" });
    const platformButtons = platform.locator(".button-group > .icon-button");
    await expect(platformButtons).toHaveCount(2);
    await expect(platformButtons.nth(0)).toHaveAccessibleName("Copy Platform & Architecture");
    await expect(platformButtons.nth(1)).toHaveAccessibleName("Filter reports by Platform & Architecture");
    const sizes = await platformButtons.evaluateAll((elements) =>
      elements.map((element) => {
        const box = element.getBoundingClientRect();
        return [Math.round(box.width), Math.round(box.height)];
      }));
    expect(sizes[0]).toEqual(sizes[1]);
    await hoverValueOf(platformButtons.nth(0));
    await platformButtons.nth(0).click();
    await expect(platformButtons.nth(0)).toHaveAttribute("data-copy-state", "copied");
    expect(await clipboard()).toBe("macOS · arm64");
    await expect(platformButtons.nth(0)).not.toHaveAttribute("data-copy-state");

    // A diagnostic field copies exactly the value it shows.
    const commitCopy = page.getByRole("button", { name: "Copy Git commit" });
    await hoverValueOf(commitCopy);
    await commitCopy.click();
    expect(await clipboard()).toBe("654cf9b187384fa8855eac4fbafaa70e75497083");
    const unknownValue = page.locator(".report-field-value").filter({ hasText: "<script>" });
    await unknownValue.hover();
    await unknownValue.getByRole("button", { name: /^Copy / }).click();
    expect(await clipboard()).toBe(await unknownValue.locator("[data-copy-source]").innerText());
    expect(await clipboard()).toContain("<script>");

    // A stack trace copies its original text and has no filter button.
    const stackHeader = page.locator(".report-field-header").filter({
      has: page.getByRole("heading", { name: "Stack trace" }),
    });
    await expect(stackHeader.locator(".button-group > .icon-button")).toHaveCount(1);
    await stackHeader.getByRole("button", { name: "Copy Stack trace" }).click();
    const rawStack = await page.locator(".stack-raw-text").evaluate((pre) => pre.textContent);
    expect(await clipboard()).toBe(rawStack);
    expect(await clipboard()).toContain("Native stack (binary build ID, object address):");
  });

  test("shows value buttons on hover or tap, except where they are always needed", async ({ page }) => {
    await page.goto(`/reports/${reportId}`);
    const overview = page.getByRole("region", { name: "Overview" });
    const platform = overview.locator(".definition-list > div").filter({ hasText: "Platform" });
    const platformGroup = platform.locator(".button-group");
    const shown = (group: Locator, visible: boolean) => expect(group).toHaveCSS("opacity", visible ? "1" : "0");

    await page.mouse.move(0, 0);
    await shown(platformGroup, false);
    // Anywhere in the field's area counts, the label included.
    await platform.locator("dt").hover();
    await shown(platformGroup, true);

    // A tap, which has no hover, shows the buttons of its value and hides those of others.
    await page.mouse.move(0, 0);
    await shown(platformGroup, false);
    await platform.locator("dt").dispatchEvent("pointerdown", { pointerType: "touch" });
    await shown(platformGroup, true);
    await page.locator("h1").first().dispatchEvent("pointerdown", { pointerType: "touch" });
    await shown(platformGroup, false);

    // The stack trace keeps its copy button, and attachments their buttons.
    const stackCopy = page.getByRole("button", { name: "Copy Stack trace" });
    await shown(stackCopy.locator("xpath=.."), true);
    for (const button of await page.locator(".attachment-actions .icon-button, .attachment-actions a").all()) {
      await expect(button).toBeVisible();
    }
  });

  test("joins the attachment buttons", async ({ page }) => {
    await page.goto(`/reports/${reportId}`);

    const buttons = page.locator(".attachment-actions.button-group").first().locator("> .icon-button");
    await expect(buttons).toHaveCount(2);
    const [first, second] = await buttons.evaluateAll((elements) =>
      elements.map((element) => {
        const style = getComputedStyle(element);
        const box = element.getBoundingClientRect();
        return {
          left: box.left,
          right: box.right,
          topLeft: style.borderTopLeftRadius,
          topRight: style.borderTopRightRadius,
        };
      }));
    expect(first.topRight).toBe("0px");
    expect(second.topLeft).toBe("0px");
    expect(first.topLeft).not.toBe("0px");
    expect(second.topRight).not.toBe("0px");
    expect(second.left).toBeLessThanOrEqual(first.right);
  });

  test("keeps report rows readable on desktop and mobile", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto("/");
    await openGroups(page);
    const row = page.locator(".report-table tbody tr:not(.report-group-row)").first();
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
    await openGroups(page);

    const legacyReport = page.locator(".report-table tbody tr:not(.report-group-row)")
      .filter({ hasText: "macOS · arm64 · Ladybird Nightly 2026-09-15" }).first();
    await legacyReport.getByRole("link", {
      name: "Crash: WebContent::ConnectionFromClient::debug_request",
    }).click();

    const fields = page.locator(".definition-list > div");
    await expect(fields).toHaveCount(5);
    await expect(fields.nth(3).locator("dt")).toHaveText("Submitted");
    await expect(fields.nth(4).locator("dt")).toHaveText("Build");

    const submitted = await fields.nth(3).boundingBox();
    const build = await fields.nth(4).boundingBox();
    expect(submitted).not.toBeNull();
    expect(build).not.toBeNull();
    expect(build!.width).toBeGreaterThanOrEqual(submitted!.width - 2);
    // A line, the grid's gap, separates the two rows.
    expect(build!.y - (submitted!.y + submitted!.height)).toBeCloseTo(1, 0);

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

    for (const asset of ["application.css", "application.js", "reports.js", "list-search.js", "filter-bar.js", "github-icon.svg", "ladybird-mark.png"]) {
      const response = await request.get(`${assetBase}${asset}`);

      expect(response.status()).toBe(200);
      expect(response.headers()["cache-control"]).toBe("public, max-age=31536000, immutable");
      expect(response.headers()["etag"]).toMatch(/^\"[0-9a-f]{64}\"$/);
    }
    expect((await request.get(`${assetBase}ladybird-mark.png`)).headers()["content-type"])
      .toBe("image/png");

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
      .toHaveValue("Web compat: https://example.test/checkout");
    // The report keeps its own title, which does not follow the one proposed for GitHub.
    await expect(page.locator(".page-header h1"))
      .toHaveText("Web compatibility: https://example.test/checkout");
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

    await search.fill("state:rejected");
    await page.waitForTimeout(800);
    await expect(search).toHaveValue("state:rejected");
    expect(new URL(page.url()).searchParams.has("q")).toBe(false);
    await search.press("Enter");

    await expect.poll(() => new URL(page.url()).searchParams.get("q"))
      .toBe("state:rejected");
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
    await openGroups(page);
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
    const reports = page.locator("tbody tr:not(.report-group-row)");
    await expect(reports).toHaveCount(2);
    await expect(reports.nth(0)).toContainText("macOS");
    await expect(reports.nth(1)).toContainText("macOS");

    await search.fill("platform:linux");
    await search.blur();
    await expect.poll(() => new URL(page.url()).searchParams.get("q"))
      .toBe("platform:linux");
    await expect(reports).toHaveCount(2);

    await search.fill("plat");
    await expect(page.getByRole("option", { name: /^platform:/ })).toBeVisible();
    await search.fill("state:triage platform:macos platform:linux");
    await search.press("Enter");
    await expect.poll(() => new URL(page.url()).searchParams.get("q"))
      .toBe("state:triage platform:macos platform:linux");
    await expect(reports).toHaveCount(4);
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
    const reports = page.locator("tbody tr:not(.report-group-row)");
    await expect(reports).toHaveCount(50);
    const initialUrl = page.url();
    await page.getByRole("button", { name: "Show more…" }).click();
    await expect(reports).toHaveCount(58);
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
    await expect(confirmation.getByRole("button", { name: "Cancel" })).toBeFocused();
    await expect(confirmation.getByRole("checkbox")).toBeChecked();
    await expect(confirmation.getByRole("checkbox", {
      name: "Reject all triage reports from this IP",
    })).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(confirmation).toBeHidden();

    await page.getByRole("button", { name: "Block submission IP" }).click();
    await confirmation.getByRole("button", { name: "Close dialog" }).click();
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
      const offset = element.value.indexOf('"authorization_team"');
      element.setSelectionRange(offset, offset);
      element.dispatchEvent(new Event("select", { bubbles: true }));
    });
    await expect(page.getByRole("heading", { name: "GitHub access team" })).toBeVisible();
    await expect(editor).toHaveValue(/"authorization_team": "LadybirdBrowser\/maintainers"/);
  });

  test("explains one setting for a name that occurs under more than one object", async ({ page }) => {
    await page.goto("/settings");
    const editor = page.getByLabel("Configuration JSON");
    const entries = page.locator("[data-setting-help-entry]:visible");

    // The same names are used for the Discord webhooks and the GitHub labels of both kinds of report.
    const caretAfter = (parent: string, name: string) => editor.evaluate(
      (element: HTMLTextAreaElement, [parent, name]: string[]) => {
        const start = element.value.indexOf(`"${parent}"`);
        const offset = element.value.indexOf(`"${name}"`, start);
        element.focus();
        element.setSelectionRange(offset, offset);
        element.dispatchEvent(new Event("select", { bubbles: true }));
      },
      [parent, name],
    );

    for (const [parent, name, path] of [
      ["webhook_urls", "web_compat", "discord.webhook_urls.web_compat"],
      ["labels", "web_compat", "github.labels.web_compat"],
      ["webhook_urls", "crash", "discord.webhook_urls.crash"],
      ["labels", "crash", "github.labels.crash"],
    ]) {
      await caretAfter(parent, name);
      await expect(entries).toHaveCount(1);
      await expect(entries.locator(".eyebrow")).toHaveText(path);
    }

    // A label in the list belongs to the setting of the list.
    await editor.evaluate((element: HTMLTextAreaElement) => {
      const offset = element.value.indexOf('"web compatibility"');
      element.setSelectionRange(offset, offset);
      element.dispatchEvent(new Event("select", { bubbles: true }));
    });
    await expect(entries).toHaveCount(1);
    await expect(entries.locator(".eyebrow")).toHaveText("github.labels.web_compat");
  });

  test("shares a report as YAML through a link that works without signing in", async ({ page, playwright }) => {
    await page.goto(`/reports/${reportId}`);

    // One click: no confirmation, and the link is in a dialog with its copy button.
    await page.getByRole("button", { name: "Share YAML" }).click();
    const dialog = page.getByRole("dialog", { name: "Share YAML" });
    await expect(dialog).toBeVisible();
    const link = dialog.getByRole("textbox", { name: "Share link" });
    await expect(link).toHaveValue(new RegExp(`^${adminUrl}/share/[A-Za-z0-9_-]{43}$`));
    await expect(dialog.getByRole("button", { name: "Copy share link" })).toBeVisible();
    const url = await link.inputValue();

    // The link is not shown again, and a link is not listed or revoked anywhere on the page.
    await dialog.getByRole("button", { name: "Close dialog" }).click();
    await page.reload();
    await expect(page.getByRole("button", { name: "Revoke" })).toHaveCount(0);
    await expect(page.getByText(url)).toHaveCount(0);

    // Opened by someone who is not signed in.
    const anonymous = await playwright.request.newContext();
    const shared = await anonymous.get(url);
    expect(shared.status()).toBe(200);
    expect(shared.headers()["content-type"]).toBe("text/plain; charset=utf-8");
    expect(shared.headers()["cache-control"]).toBe("private, no-store");
    expect(shared.headers()["content-encoding"]).toBeUndefined();
    const document = await shared.text();
    expect(document).toContain("untrusted");
    expect(document).toContain(`id: ${reportId}`);
    // Fields are keys of their own, with the whole page address, which the public GitHub draft leaves the query out of.
    expect(document).toMatch(/^fields:\n(?:  .*\n|\n)*  url: https:\/\/example\.test\/checkout\?step=payment$/m);
    expect(document).toContain("name: crash-diagnostics.txt");

    const attachmentUrl = (name: string) =>
      document.match(new RegExp(`name: ${name}\\n[\\s\\S]*?url: (\\S+)`))![1];
    const screenshot = await anonymous.get(attachmentUrl("screenshot.png"));
    expect(screenshot.status()).toBe(200);
    expect(screenshot.headers()["content-type"]).toBe("image/png");
    const diagnostics = await anonymous.get(attachmentUrl("crash-diagnostics.txt"));
    expect(diagnostics.status()).toBe(200);
    expect(await diagnostics.text()).toContain("Ladybird crash report, format 1");

    // Nothing else opens: unknown, malformed and misplaced tokens all look the same.
    for (const path of [`/share/${"a".repeat(43)}`, "/share/short", `/share/${"a".repeat(43)}/attachments/${reportId}`]) {
      const response = await anonymous.get(`${adminUrl}${path}`);
      expect(response.status(), path).toBe(404);
    }
    expect((await anonymous.get(`${url}/attachments/${reportId}`)).status()).toBe(404);

    // Creating a link is in the activity of the report.
    await expect(page.getByText("report.share_create")).toBeVisible();
    await anonymous.dispose();
  });

  test("opens account actions in a popover", async ({ page }) => {
    await page.goto("/");

    await page.getByRole("button", { name: "Open account menu for browser-tester" }).click();
    const signOut = page.getByRole("button", { name: "Sign out" });
    await expect(signOut).toBeVisible();
    expect(await signOut.evaluate((button) => getComputedStyle(button).borderTopWidth)).toBe("0px");
  });

  test.describe("grouping reports by stack signature", () => {
    // 52 reports of two kinds that share one stack: two signatures, 26 reports each, with the last two of them on the
    // second page.
    const fixtureReports = '/?q=version%3A"Pagination fixture"';
    const groups = (page: Page) => page.locator(".report-group-row");
    const toggle = (page: Page, index: number) => groups(page).nth(index).getByRole("button");

    test("folds reports with the same signature into collapsed groups", async ({ page }) => {
      await page.goto(fixtureReports);

      await expect(groups(page)).toHaveCount(2);
      await expect(toggle(page, 0)).toHaveAttribute("aria-expanded", "false");
      await expect(toggle(page, 0)).toContainText("25 reports");
      await expect(toggle(page, 1)).toContainText("25 reports");

      // Only the headers show, and the first one is the most recent report.
      await expect(page.locator("tbody tr:visible")).toHaveCount(2);
      await expect(page.locator(".report-group-member")).toHaveCount(50);
      await expect(groups(page).nth(0)).toContainText("Crash");
      await expect(groups(page).nth(1)).toContainText("Web compatibility");
    });

    test("opens a group to its reports, which open as usual", async ({ page }) => {
      await page.goto(fixtureReports);

      await toggle(page, 0).click();
      await expect(toggle(page, 0)).toHaveAttribute("aria-expanded", "true");
      await expect(page.locator("tbody tr:visible")).toHaveCount(2 + 25);

      await page.locator(".report-group-member:visible .report-title-link").nth(3).click();
      await expect(page).toHaveURL(/\/reports\/[0-9a-f-]{36}$/);

      await page.goBack();
      await toggle(page, 0).click();
      await toggle(page, 0).click();
      await expect(page.locator("tbody tr:visible")).toHaveCount(2);
    });

    test("adds the reports of the next page to the groups that are already there", async ({ page }) => {
      await page.goto(fixtureReports);
      await toggle(page, 1).click();
      await page.getByRole("button", { name: "Show more…" }).click();

      await expect(toggle(page, 0)).toContainText("26 reports");
      await expect(toggle(page, 1)).toContainText("26 reports");
      await expect(groups(page)).toHaveCount(2);
      await expect(page.locator(".report-group-member")).toHaveCount(52);

      // The group that was open takes its new report open, the other does not.
      await expect(toggle(page, 0)).toHaveAttribute("aria-expanded", "false");
      await expect(toggle(page, 1)).toHaveAttribute("aria-expanded", "true");
      await expect(page.locator("tbody tr:visible")).toHaveCount(2 + 26);
    });

    test("opens a list that is a single group", async ({ page }) => {
      await page.goto('/?q=version%3A"Pagination fixture" kind%3Acrash');

      await expect(groups(page)).toHaveCount(1);
      await expect(toggle(page, 0)).toHaveAttribute("aria-expanded", "true");
      await expect(page.locator("tbody tr:visible")).toHaveCount(1 + 26);
    });

    test("groups the results of a search", async ({ page }) => {
      await page.goto("/");
      const search = page.getByLabel("Search reports");
      await search.fill('version:"Pagination fixture"');
      await search.press("Enter");

      await expect(groups(page)).toHaveCount(2);
      await expect(toggle(page, 0)).toContainText("25 reports");
    });
  });

  test("tells confirmed reports from resolved ones", async ({ page }) => {
    // The report of a resolved issue is resolved, and not in the default list.
    await page.goto("/");
    await expect(page.locator("tbody .badge-resolved")).toHaveCount(0);

    await page.goto("/?q=state%3Aresolved");
    const resolved = page.locator("tbody .badge-resolved");
    await expect(resolved).toHaveCount(1);
    await expect(resolved).toHaveText("Resolved");
    await expect(page.locator("tbody tr")).toHaveCount(1);
    const color = (pill: Locator) => pill.evaluate((element) => getComputedStyle(element).color);
    const resolvedColor = await color(resolved);

    await page.goto("/?q=state%3Aconfirmed");
    const confirmed = page.locator("tbody .badge-confirmed").first();
    await expect(confirmed).toHaveText("Confirmed");

    // Being tracked is not being done: the pills do not share a colour.
    expect(await color(confirmed)).not.toBe(resolvedColor);
  });

  test("rejects and restores reports while retaining their details", async ({ page }) => {
    await page.goto("/?q=state%3Atriage+platform%3APaginationOS");
    await openGroups(page);
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
    // The draft refers to the report's screenshot with a placeholder.
    await expect(page.getByLabel("Description", { exact: true }))
      .toHaveValue(/\n## Screenshot\n\n\{screenshot\}\n/);
    await page.getByLabel("Description", { exact: true })
      .fill("Created by the browser test.\n\n{screenshot}");
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
    // The screenshot is fetched through this service, since GitHub redirects it to storage the policy cannot name.
    const screenshot = page.locator(".issue-description img");
    await expect(screenshot).toHaveAttribute(
      "src",
      "/github-assets/00000000-0000-4000-8000-000000000001",
    );
    await expect.poll(() =>
      screenshot.evaluate((image: HTMLImageElement) => image.complete ? image.naturalWidth : 0),
    ).toBeGreaterThan(0);
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
      `${fakeGithub}/test/issue-field/7300`,
    );
    expect((await issueField.json()).value)
      .toBe(`${adminUrl}/issues/${createdIssueId}`);

    const createdIssue = await page.request.get(
      `${fakeGithub}/test/latest-created-issue`,
    );
    expect((await createdIssue.json()).labels).toEqual(["web compatibility"]);
    expect((await createdIssue.json()).body).toBe(
      // Browsers submit line breaks in a textarea as CRLF.
      "Created by the browser test.\r\n\r\n" +
      "![Screenshot of the page in the report]" +
      "(https://github.com/user-attachments/assets/00000000-0000-4000-8000-000000000001)",
    );
    const upload = await (await page.request.get(`${fakeGithub}/test/latest-upload`)).json();
    expect(upload).toMatchObject({
      content_type: "image/png",
      repository_id: "4242",
      authorized: true,
    });
    expect(upload.size).toBeGreaterThan(0);

    await page.getByRole("link", { name: "Reports", exact: true }).click();
    await page.getByLabel("Search reports").fill("state:triage");
    await page.getByLabel("Search reports").press("Enter");
    await expect(page.locator(`a[href="/reports/${reportId}"]`)).toHaveCount(0);

    await page.getByLabel("Search reports").fill("state:confirmed");
    await page.getByLabel("Search reports").press("Enter");
    await expect.poll(() => new URL(page.url()).searchParams.get("q"))
      .toBe("state:confirmed");
    // A list that updates again collapses its groups, so keep opening them until the report shows.
    await expect(async () => {
      await openGroups(page);
      await expect(page.locator(`a[href="/reports/${reportId}"]`)).toBeVisible({ timeout: 1000 });
    }).toPass();
  });

  test("shows the audit log", async ({ page }) => {
    await page.goto("/operations");

    await expect(
      page.getByRole("navigation", { name: "Main navigation" }).getByRole("link", { name: "Audit log" }),
    ).toHaveAttribute("aria-current", "page");
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

  test("shows the audit columns in order and expands details on click", async ({ page }) => {
    await page.goto("/operations");

    await expect(page.locator(".audit-table th")).toHaveText(["Time", "Action", "Actor", "Target", "Details"]);

    const details = page.locator(".event-details").first();
    await expect(details).toHaveAttribute("title", /.+/);
    const collapsed = await details.evaluate((element) => element.getBoundingClientRect().height);
    await details.click();
    await expect(details).toHaveAttribute("aria-expanded", "true");
    expect(await details.evaluate((element) => getComputedStyle(element).textOverflow)).toBe("ellipsis");
    expect(await details.evaluate((element) => getComputedStyle(element).whiteSpace)).toBe("normal");
    expect(await details.evaluate((element) => element.getBoundingClientRect().height)).toBeGreaterThanOrEqual(
      collapsed,
    );
    await details.click();
    await expect(details).toHaveAttribute("aria-expanded", "false");
  });

  test("loads older audit events without reloading the page", async ({ page }) => {
    await page.goto("/operations");

    const rows = page.locator("[data-rows] tr");
    const history = rows.filter({ hasText: "fixture.history" });
    const showMore = page.getByRole("button", { name: "Show more…" });

    await expect(rows).toHaveCount(100);
    await expect(showMore).toBeVisible();

    // Prove the page is not reloaded: this marker disappears on navigation.
    await page.evaluate(() => {
      (window as unknown as { auditLogMarker: boolean }).auditLogMarker = true;
    });
    const firstRow = await rows.first().innerText();
    const historyBefore = await history.count();

    // Earlier tests add their own events, so keep loading until the last page.
    let pages = 0;
    while (await showMore.count() > 0) {
      const before = await rows.count();
      await showMore.click();
      await expect.poll(() => rows.count()).toBeGreaterThan(before);
      pages += 1;
      expect(pages).toBeLessThan(10);
    }

    expect(pages).toBeGreaterThanOrEqual(1);
    expect(await history.count()).toBeGreaterThan(historyBefore);
    await expect(rows.first()).toHaveText(firstRow);
    expect(await page.evaluate(
      () => (window as unknown as { auditLogMarker?: boolean }).auditLogMarker,
    )).toBe(true);
    await expect(page).toHaveURL(/\/operations$/);

    // Page boundaries neither skip nor repeat events.
    await expect(history).toHaveCount(130);
    const sequences = await history.locator("code").allInnerTexts();
    expect(new Set(sequences).size).toBe(130);
  });

  test("links an exact issue match and unlinks it from the report", async ({ page }) => {
    await page.goto("/issues");
    await page.getByRole("link", { name: "Intermittent navigation timeout" }).click();

    const matches = page.getByRole("region", { name: "Potential matches" });
    await expect(matches).toBeVisible();
    const candidate = matches.getByRole("link", { name: /WebContent::/ }).first();
    const candidateUrl = await candidate.getAttribute("href");
    expect(candidateUrl).toMatch(/^\/reports\/[0-9a-f-]+$/);

    // Linking and unlinking happen in place: this marker is lost if the page reloads.
    const stillSamePage = () => page.evaluate(
      () => (window as unknown as { samePage?: boolean }).samePage === true,
    );
    await page.evaluate(() => {
      (window as unknown as { samePage: boolean }).samePage = true;
    });
    const assigned = page.getByRole("region", { name: "Assigned reports" });
    await matches.getByRole("button", { name: /Link report/ }).first().click();
    await expect(assigned.locator(`a[href="${candidateUrl}"]`)).toBeVisible();
    await expect(matches.locator(`a[href="${candidateUrl}"]`)).toHaveCount(0);
    expect(await stillSamePage()).toBe(true);

    await assigned.getByRole("row").filter({ has: page.locator(`a[href="${candidateUrl}"]`) })
      .getByRole("button", { name: /Unlink report/ }).click();
    await expect(assigned.locator(`a[href="${candidateUrl}"]`)).toHaveCount(0);
    await expect(matches.locator(`a[href="${candidateUrl}"]`)).toBeVisible();
    expect(await stillSamePage()).toBe(true);

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
      `${fakeGithub}/test/issue-field/4812`,
    );
    expect((await existingIssueField.json()).value)
      .toBe(`${adminUrl}/issues/${trackedIssueId}`);

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
    await expect(page.locator(`tr:has(a[href="/issues/${trackedIssueId}"])`)
      .getByText("Resolved", { exact: true })).toBeVisible();

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
    // The form posts in the background, so wait for the report to leave the matches before leaving the page.
    await expect(matches.locator(`a[href="${candidateUrl}"]`)).toHaveCount(0);
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
    await page.request.post(`${fakeGithub}/test/field-visibility`, {
      data: { visibility: "all" },
    });
    await linkDialog.getByRole("button", { name: "Link issue" }).click();
    await expect(page.getByRole("link", { name: "Issue #6200" })).toBeVisible();
    await expect(page.getByText("The Reports link could not be added"))
      .toBeVisible();
    const publicField = await page.request.get(
      `${fakeGithub}/test/issue-field/6200`,
    );
    expect((await publicField.json()).value).toBeNull();

    await page.request.post(`${fakeGithub}/test/field-visibility`, {
      data: { visibility: "organization_members_only" },
    });
    await page.reload();
    const replacementIssueField = await page.request.get(
      `${fakeGithub}/test/issue-field/6200`,
    );
    expect((await replacementIssueField.json()).value)
      .toBe(`${adminUrl}/issues/${trackedIssueId}`);
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
    const unlink = page.getByRole("button", { name: `Unlink report ${reportId}` });
    await unlink.click();
    // The form posts in the background, so wait for the page to show the result before leaving it.
    await expect(unlink).toHaveCount(0);

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
      `${fakeGithub}/repos/LadybirdBrowser/ladybird/issues/7300`,
    );
    expect((await githubIssue.json()).state).toBe("open");
  });

  // Late, as it adds a second resolved report.
  test("adds a report to a resolved issue and resolves it", async ({ page }) => {
    await page.goto("/?q=state%3Atriage+platform%3APaginationOS");
    await openGroups(page);
    const reportPath = await page.locator(".report-title-link").first().getAttribute("href");
    await page.goto(reportPath!);
    await page.getByRole("button", { name: "Add report to issue" }).click();

    const issueSearch = page.getByRole("combobox", { name: "Search issues" });
    await issueSearch.fill("Fixed in a nightly");
    const issuePopover = page
      .locator('[data-search-url="/api/issue-options"]')
      .locator("[data-entity-popover]");
    await expect(issuePopover.getByText("Resolved issues", { exact: true })).toBeVisible();
    const option = page.getByRole("option", { name: /Fixed in a nightly/ });
    await expect(option).toContainText("GitHub issue #5000");
    await option.click();
    await page.getByRole("dialog", { name: "Add report to an issue" })
      .getByRole("button", { name: "Add report", exact: true })
      .click();
    await expect(page).toHaveURL(/\/issues\/[0-9a-f-]+$/);
    await expect(page.getByRole("heading", { name: "Fixed in a nightly" })).toBeVisible();

    await page.goto(reportPath!);
    await expect(page.locator(".badge-resolved")).toHaveText("Resolved");
  });

  test("applies an action to every selected report at once", async ({ page }) => {
    // Four rejected reports, in two groups: two crashes and two web compatibility reports.
    const bulkReports = (state: string) => `/?q=state%3A${state}+version%3A%22Bulk+fixture%22`;
    const bar = page.getByRole("region", { name: /selected$/ });
    const reports = page.locator("tbody tr:not(.report-group-row)");
    const selectAll = page.getByLabel("Select every report in the list");
    const crashGroup = page.locator(".report-group-row", { hasText: "Crash" }).getByRole("checkbox");

    await page.goto(bulkReports("rejected"));
    await expect(reports).toHaveCount(4);
    await expect(bar).toBeHidden();

    // A group's checkbox selects its reports, and the list's checkbox selects every report.
    await crashGroup.check();
    await expect(bar).toContainText("2 reports selected");
    await expect(selectAll).toHaveJSProperty("indeterminate", true);
    await selectAll.check();
    await expect(bar).toContainText("4 reports selected");
    await expect(crashGroup).toBeChecked();

    // Back to triage asks first, then restores rejected reports that aren't linked to an issue.
    await bar.getByRole("button", { name: "Back to triage" }).click();
    const triage = page.getByRole("dialog", { name: "Return 4 reports to triage?" });
    await triage.getByRole("button", { name: "Back to triage" }).click();
    await expect(page.getByText("No matching reports")).toBeVisible();
    await expect.poll(() => new URL(page.url()).searchParams.get("q"))
      .toBe('state:rejected version:"Bulk fixture"');

    // Adding reports to an issue confirms them, and opens the issue (which an earlier test merged into another).
    await page.goto(bulkReports("triage"));
    await crashGroup.check();
    await bar.getByRole("button", { name: "Add to issue…" }).click();
    const dialog = page.getByRole("dialog", { name: "Add 2 reports to an issue" });
    await dialog.getByRole("combobox", { name: "Search issues" }).fill("navigation timeout");
    await page.getByRole("option", { name: /Intermittent navigation timeout/ }).click();
    await dialog.getByRole("button", { name: "Add reports", exact: true }).click();
    await expect(page).toHaveURL(/\/issues\/[0-9a-f-]+$/);
    await page.goto(bulkReports("confirmed"));
    await expect(reports).toHaveCount(2);

    // Reject asks first, then rejects every selected report, linked or not.
    await page.goto(bulkReports("triage+state%3Aconfirmed"));
    await selectAll.check();
    await bar.getByRole("button", { name: "Reject" }).click();
    await page.getByRole("dialog", { name: "Reject 4 reports?" }).getByRole("button", { name: "Reject" }).click();
    await expect(page.getByText("No matching reports")).toBeVisible();

    // Confirming needs an issue link for every report; when one lacks it, nothing changes and the bar says why.
    await page.goto(bulkReports("rejected"));
    await selectAll.check();
    await bar.getByRole("button", { name: "Confirm" }).click();
    await expect(bar.getByRole("alert")).toHaveText("Add the selected reports to an issue to confirm them");
    await expect(bar).toContainText("4 reports selected");
    await page.reload();
    await expect(reports).toHaveCount(4);

    await crashGroup.check();
    await bar.getByRole("button", { name: "Confirm" }).click();
    await expect(reports).toHaveCount(2);
    await page.goto(bulkReports("confirmed"));
    await expect(reports).toHaveCount(2);
  });
});

// Last, because it waits for a sweep of the background task. The fake GitHub issues tokens that expire inside the
// refresh window, so the task rotates them.
test("rotates GitHub tokens in the background", async ({ page }) => {
  test.setTimeout(120_000);
  const refreshCount = async () =>
    (await (await page.request.get(`${fakeGithub}/test/token-refresh-count`)).json()).count;

  await page.goto("/login");
  await page.getByRole("link", { name: "Continue with GitHub" }).click();
  await expect(page).toHaveURL(`${adminUrl}/`);

  const afterSignIn = await refreshCount();
  await expect.poll(refreshCount, { timeout: 90_000 }).toBeGreaterThan(afterSignIn);

  await page.reload();
  await expect(page).toHaveURL(`${adminUrl}/`);
});
