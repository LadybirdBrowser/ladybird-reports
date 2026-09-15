import { expect, test } from "@playwright/test";

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
    await expect(reportLink).toHaveText("Web compatibility report");
    await expect(page.locator("tbody")).not.toContainText(reportId);
    await expect(reportLink.locator("xpath=ancestor::tr"))
      .toContainText(/\d{2} [A-Z][a-z]{2} \d{4}, \d{2}:\d{2} UTC/);

    await reportLink.click();
    await expect(page.getByRole("heading", { name: "Native stack" })).toBeVisible();
    await expect(page.getByText("Core::ThreadEventQueue::process()")).toBeVisible();
    await expect(page.getByText("macOS", { exact: true }).first()).toBeVisible();
    await expect(page.getByText("arm64", { exact: true }).first()).toBeVisible();
    await expect(page.getByText("127.0.0.1", { exact: true })).toBeVisible();
    await expect(page.getByText("Unknown field", { exact: true })).toBeVisible();
    await expect(page.locator(".report-field-value").filter({ hasText: "<script>" })).toBeVisible();
    await expect(page.getByRole("link", { name: "Filter reports by Native stack" })).toHaveCount(0);
    await expect(page.getByRole("heading", { name: "Attachments" }).locator(".."))
      .toContainText("0 files");
    await expect(page.getByText("No attachments")).toHaveCount(0);

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

    for (const asset of ["application.css", "application.js"]) {
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

  test("creates issues from reports and selects an existing GitHub issue", async ({ page }) => {
    await page.goto("/issues");
    await expect(page.getByRole("heading", { name: "Create issue" })).toHaveCount(0);
    await expect(page.getByText("Intermittent navigation timeout")).toBeVisible();
    await expect(page.getByRole("button", { name: /Apply/ })).toHaveCount(0);

    await page.getByLabel("Include resolved").check();
    await expect.poll(() => new URL(page.url()).searchParams.get("resolved"))
      .toBe("true");

    await page.route("**/api/github-issue-options**", async (route) => {
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({
          results: [
            {
              value: "4812",
              label: "Fix overlapping navigation controls",
              description: "https://github.com/LadybirdBrowser/ladybird/issues/4812",
              identifier: "#4812",
              badge: "Linked in Reports",
              badge_tone: "assigned",
              footnote: "Already tracked as Intermittent navigation timeout",
            },
          ],
        }),
      });
    });

    await page.goto(`/reports/${reportId}`);
    await page.getByText("Create a new issue from this report").click();
    await page.getByText("Link an existing issue").click();
    await expect(page.getByLabel("GitHub title")).toBeHidden();
    await expect(page.getByLabel("GitHub body")).toBeHidden();

    const issueSearch = page.getByRole("combobox", { name: "Search GitHub issues" });
    await issueSearch.fill("navigation");

    const issuePopover = page.locator("[data-entity-popover]");
    await expect(issuePopover).toBeVisible();
    expect(await issuePopover.evaluate((element) => element.matches(":popover-open"))).toBe(true);

    const [popoverBounds, actionCardBounds] = await Promise.all([
      issuePopover.boundingBox(),
      page.locator(".action-card").boundingBox(),
    ]);
    expect(popoverBounds).not.toBeNull();
    expect(actionCardBounds).not.toBeNull();
    expect(popoverBounds!.x).toBeLessThan(actionCardBounds!.x);

    const matchingIssue = page.getByRole("option", {
      name: /Fix overlapping navigation controls/,
    });
    await expect(matchingIssue).toBeVisible();
    await expect(matchingIssue).toContainText("Linked in Reports");
    await expect(matchingIssue).toContainText(
      "Already tracked as Intermittent navigation timeout",
    );
    await matchingIssue.click();

    await expect(page.locator(".entity-selector-chip")).toContainText(
      "Fix overlapping navigation controls",
    );
    await expect(page.locator('input[name="github_issue_number"]')).toHaveValue("4812");

    await page.getByText("Create a new GitHub issue").click();
    await expect(page.getByLabel("GitHub title")).toBeVisible();
    await expect(page.getByLabel("GitHub body")).toBeVisible();
  });

  test("uses a custom popover for select controls", async ({ page }) => {
    await page.goto(`/reports/${reportId}`);

    const issueSelect = page.getByRole("combobox", { name: "Issue" });
    await expect(issueSelect).toHaveText("Leave in triage");
    await issueSelect.click();

    const listbox = page.getByRole("listbox", { name: "Issue" });
    await expect(listbox).toBeVisible();
    await page.getByRole("option", { name: "Intermittent navigation timeout" }).click();
    await expect(issueSelect).toHaveText("Intermittent navigation timeout");
  });

  test("searches reports with qualified syntax", async ({ page }) => {
    await page.goto("/");
    await expect(page.getByLabel("Search reports")).toHaveValue("state:triage");
    await expect(page.getByRole("button", { name: "Apply filters" })).toHaveCount(0);
    await page.getByLabel("Search reports").fill("state:triage platform:macos kind:crash");

    await expect.poll(() => new URL(page.url()).searchParams.get("q"))
      .toBe("state:triage platform:macos kind:crash");
    await expect(page.locator("tbody tr")).toHaveCount(2);
    await expect(page.locator("tbody tr").nth(0)).toContainText("macOS");
    await expect(page.locator("tbody tr").nth(1)).toContainText("macOS");
  });

  test("blocks and unblocks the submission source from the report actions", async ({ page }) => {
    await page.goto(`/reports/${reportId}`);

    await expect(page.getByRole("heading", { name: "Actions" })).toBeVisible();
    await page.getByRole("button", { name: "Block IP" }).click();
    const confirmation = page.getByRole("dialog", { name: "Block this IP?" });
    await expect(confirmation).toBeVisible();
    await expect(confirmation.getByRole("checkbox")).toBeChecked();
    await confirmation.getByRole("button", { name: "Cancel" }).click();
    await expect(confirmation).toBeHidden();

    await page.getByRole("button", { name: "Block IP" }).click();
    await confirmation.getByRole("checkbox").uncheck();
    await confirmation.getByRole("button", { name: "Block IP" }).click();
    await expect(page.getByText("IP is blocked indefinitely")).toBeVisible();

    await page.getByRole("button", { name: "Unblock IP" }).click();
    await expect(page.getByRole("button", { name: "Block IP" })).toBeVisible();
  });

  test("reorders recognized fields by dragging rows", async ({ page }) => {
    await page.goto("/settings");

    await expect(page.getByRole("columnheader", { name: "Position" })).toHaveCount(0);

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

    await expect(page.getByRole("button", { name: "Save field order" })).toHaveCount(0);
    await expect(page.getByText("Native stack is now at position 2.")).toBeVisible();
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
  });

  test("opens account actions in a popover", async ({ page }) => {
    await page.goto("/");

    await page.getByRole("button", { name: "Open account menu for browser-tester" }).click();
    await expect(page.getByText("Signed in as")).toHaveCount(0);
    const signOut = page.getByRole("button", { name: "Sign out" });
    await expect(signOut).toBeVisible();
    expect(await signOut.evaluate((button) => getComputedStyle(button).borderTopWidth)).toBe("0px");
  });

  test("creates an issue, assigns the report, and filters it from triage", async ({ page }) => {
    await page.goto(`/reports/${reportId}`);
    await page.getByText("Create a new issue from this report").click();
    await page.getByLabel("Title", { exact: true }).fill("Renderer overlap on test page");
    await page.getByLabel("Description", { exact: true }).fill("Created by the browser test.");
    await page.getByRole("button", { name: "Create and assign" }).click();

    await expect(page).toHaveURL(/\/issues\/[0-9a-f-]+$/);
    await expect(
      page.getByRole("heading", { name: "Renderer overlap on test page" }),
    ).toBeVisible();
    await expect(page.getByRole("link", { name: reportId })).toBeVisible();

    await page.getByRole("link", { name: "Reports", exact: true }).click();
    await expect(page.locator(`a[href="/reports/${reportId}"]`)).toHaveCount(0);

    await page.getByLabel("Search reports").fill("state:assigned");
    await expect.poll(() => new URL(page.url()).searchParams.get("q"))
      .toBe("state:assigned");
    await expect(page.locator(`a[href="/reports/${reportId}"]`)).toBeVisible();
  });

  test("shows the audit log", async ({ page }) => {
    await page.goto("/operations");

    await expect(page.getByRole("heading", { name: "Audit log" })).toBeVisible();
    await expect(page.getByText("field_definitions.reordered", { exact: true })).toBeVisible();
    await expect(page.getByText("report.source_blocked", { exact: true })).toBeVisible();
    await expect(page.getByText("issue.created", { exact: true })).toBeVisible();
    await expect(page.getByText("report.submitted", { exact: true }).first()).toBeVisible();
    await expect(page.getByText("session.signed_in", { exact: true })).toBeVisible();
  });
});
