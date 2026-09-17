import { createServer } from "node:http";

const port = Number(process.env.GITHUB_TEST_PORT ?? "3101");
const issueStates = new Map();
let latestCreatedIssue;
const issueFieldValues = new Map();
let issueFieldVisibility = "organization_members_only";

function sendJson(response, status, value) {
  response.writeHead(status, { "content-type": "application/json" });
  response.end(JSON.stringify(value));
}

const server = createServer((request, response) => {
  const url = new URL(request.url, `http://127.0.0.1:${port}`);
  console.log(`${request.method} ${url.pathname}`);

  if (request.method === "GET" && url.pathname === "/health") {
    sendJson(response, 200, { status: "ok" });
    return;
  }

  if (request.method === "GET" && url.pathname === "/login/oauth/authorize") {
    const callback = new URL(url.searchParams.get("redirect_uri"));
    if (callback.origin !== "http://127.0.0.1:3100" || callback.pathname !== "/auth/callback") {
      sendJson(response, 400, { message: "Invalid callback" });
      return;
    }

    callback.searchParams.set("code", "browser-test-oauth-code");
    callback.searchParams.set("state", url.searchParams.get("state"));
    response.writeHead(302, { location: callback.toString() });
    response.end();
    return;
  }

  if (request.method === "POST" && url.pathname === "/login/oauth/access_token") {
    sendJson(response, 200, {
      access_token: "browser-test-token",
      expires_in: 3600,
    });
    return;
  }

  if (request.method === "GET" && url.pathname === "/user") {
    sendJson(response, 200, { id: 12345, login: "browser-tester" });
    return;
  }

  if (
    request.method === "GET" &&
    url.pathname === "/orgs/LadybirdBrowser/teams/maintainers/memberships/browser-tester"
  ) {
    sendJson(response, 200, { state: "active" });
    return;
  }

  if (request.method === "GET" && url.pathname === "/test/latest-created-issue") {
    sendJson(response, 200, latestCreatedIssue ?? {});
    return;
  }

  if (request.method === "POST" && url.pathname === "/test/field-visibility") {
    let body = "";
    request.setEncoding("utf8");
    request.on("data", (chunk) => {
      body += chunk;
    });
    request.on("end", () => {
      issueFieldVisibility = JSON.parse(body).visibility;
      sendJson(response, 200, { visibility: issueFieldVisibility });
    });
    return;
  }

  const testIssueField = url.pathname.match(/^\/test\/issue-field\/(\d+)$/);
  if (request.method === "GET" && testIssueField) {
    sendJson(response, 200, {
      value: issueFieldValues.get(Number(testIssueField[1])) ?? null,
    });
    return;
  }

  if (request.method === "GET" && url.pathname === "/orgs/LadybirdBrowser/issue-fields") {
    sendJson(response, 200, [{
      id: 98500,
      name: "Ladybird Reports",
      data_type: "text",
      visibility: issueFieldVisibility,
    }]);
    return;
  }

  const issueFieldMatch = url.pathname.match(
    /^\/repos\/[^/]+\/[^/]+\/issues\/(\d+)\/issue-field-values$/,
  );
  if (request.method === "POST" && issueFieldMatch) {
    let body = "";
    request.setEncoding("utf8");
    request.on("data", (chunk) => {
      body += chunk;
    });
    request.on("end", () => {
      const field = JSON.parse(body).issue_field_values[0];
      if (field.field_id !== 98500) {
        sendJson(response, 422, { message: "Invalid field" });
        return;
      }
      issueFieldValues.set(Number(issueFieldMatch[1]), field.value);
      sendJson(response, 200, [{
        issue_field_id: field.field_id,
        data_type: "text",
        value: field.value,
      }]);
    });
    return;
  }

  if (request.method === "GET" && url.pathname === "/search/issues") {
    sendJson(response, 200, {
      items: [
        {
          id: 94812,
          number: 4812,
          title: "Intermittent navigation timeout",
          body: "Reports collected while investigating navigation stalls.",
          html_url: "https://github.com/LadybirdBrowser/ladybird/issues/4812",
          state: issueStates.get(4812) ?? "open",
          updated_at: "2026-09-15T12:00:00Z",
        },
        {
          id: 96200,
          number: 6200,
          title: "Investigate renderer overlap",
          body: "Renderer overlap reproduced in the current build.",
          html_url: "https://github.com/LadybirdBrowser/ladybird/issues/6200",
          state: issueStates.get(6200) ?? "open",
          updated_at: "2026-09-15T12:00:00Z",
        },
      ],
    });
    return;
  }

  const issueMatch = url.pathname.match(/^\/repos\/[^/]+\/[^/]+\/issues\/(\d+)$/);
  if (request.method === "GET" && issueMatch) {
    const number = Number(issueMatch[1]);
    sendJson(response, 200, {
      id: number + 90000,
      number,
      title: number === 6200
        ? "Investigate renderer overlap"
        : number === 4812
          ? "Intermittent navigation timeout"
          : latestCreatedIssue?.title ?? "GitHub issue",
      body: number === 6200
        ? "Renderer overlap reproduced in the current build."
        : number === 4812
          ? "Reports collected while investigating navigation stalls."
          : latestCreatedIssue?.body ?? "",
      html_url: `https://github.com/LadybirdBrowser/ladybird/issues/${number}`,
      state: issueStates.get(number) ?? "open",
      updated_at: new Date().toISOString(),
    });
    return;
  }

  if (request.method === "POST" && /\/issues$/.test(url.pathname)) {
    let body = "";
    request.setEncoding("utf8");
    request.on("data", (chunk) => {
      body += chunk;
    });
    request.on("end", () => {
      const issue = JSON.parse(body);
      latestCreatedIssue = issue;
      issueStates.set(7300, "open");
      sendJson(response, 201, {
        id: 97300,
        number: 7300,
        title: issue.title,
        body: issue.body,
        html_url: "https://github.com/LadybirdBrowser/ladybird/issues/7300",
        state: "open",
        updated_at: "2026-09-15T12:00:00Z",
      });
    });
    return;
  }

  sendJson(response, 404, { message: "Not found" });
});

server.listen(port, "127.0.0.1");
