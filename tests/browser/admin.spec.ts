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

  const visibleText = await page.locator("body").innerText();
  expect(visibleText).not.toMatch(/maintainer|LadybirdBrowser\/maintainers/i);
  await expect(page.locator("footer")).toContainText("Diagnostic triage");
});

test.describe("authenticated management UI", () => {
  test.use({
    storageState: {
      cookies: [
        {
          name: "session",
          value: "browser-test-session",
          domain: "127.0.0.1",
          path: "/",
          httpOnly: true,
          secure: false,
          sameSite: "Lax",
          expires: -1,
        },
      ],
      origins: [],
    },
  });

  test("shows setup credentials and escapes unknown report fields", async ({ page }) => {
    const response = await page.goto("/");

    expect(response?.headers()["x-request-id"]).toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
    );
    await expect(page.getByText("Reporting database setup is incomplete.")).toBeVisible();
    await expect(page.getByRole("link", { name: reportId })).toBeVisible();

    await page.getByRole("link", { name: reportId }).click();
    await expect(page.getByRole("heading", { name: "Native stack" })).toBeVisible();
    await expect(page.getByText("Unknown field", { exact: true })).toBeVisible();
    await expect(page.locator("pre").filter({ hasText: "<script>" })).toBeVisible();

    expect(await page.evaluate(() => (window as any).fixtureWasExecuted)).toBeUndefined();
  });

  test("sets security headers and revalidates static assets", async ({ page, request }) => {
    const pageResponse = await page.goto("/settings");
    const pageHeaders = pageResponse?.headers() ?? {};

    expect(pageHeaders["cache-control"]).toBe("private, no-store");
    expect(pageHeaders["content-security-policy"]).toContain("default-src 'none'");
    expect(pageHeaders["content-security-policy"]).toContain("script-src 'self'");
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
  });

  test("blocks and unblocks the submission source from the report actions", async ({ page }) => {
    await page.goto(`/reports/${reportId}`);

    await expect(page.getByRole("heading", { name: "Actions" })).toBeVisible();
    await page.getByRole("button", { name: "Block IP" }).click();
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

    await page.getByRole("button", { name: "Save field order" }).click();
    await expect(orderedRows.nth(0)).toContainText("signal");
    await expect(orderedRows.nth(1)).toContainText("stack");
  });

  test("creates an issue, assigns the report, and filters it from triage", async ({ page }) => {
    await page.goto(`/reports/${reportId}`);
    await page.getByText("Create a new issue from this report").click();
    await page.getByLabel("Title").fill("Renderer overlap on test page");
    await page.getByLabel("Description").fill("Created by the browser test.");
    await page.getByRole("button", { name: "Create and assign" }).click();

    await expect(page).toHaveURL(/\/issues\/[0-9a-f-]+$/);
    await expect(
      page.getByRole("heading", { name: "Renderer overlap on test page" }),
    ).toBeVisible();
    await expect(page.getByRole("link", { name: reportId })).toBeVisible();

    await page.getByRole("link", { name: "Reports", exact: true }).click();
    await expect(page.getByText("No matching reports", { exact: true })).toBeVisible();

    await page.getByLabel("State").selectOption("assigned");
    await page.getByRole("button", { name: "Apply filters" }).click();
    await expect(page.getByRole("link", { name: reportId })).toBeVisible();
  });

  test("shows the audit log", async ({ page }) => {
    await page.goto("/operations");

    await expect(page.getByRole("heading", { name: "Audit log" })).toBeVisible();
    await expect(page.getByText("field_definitions.reordered", { exact: true })).toBeVisible();
    await expect(page.getByText("report.source_blocked", { exact: true })).toBeVisible();
    await expect(page.getByText("issue.created", { exact: true })).toBeVisible();
  });
});
