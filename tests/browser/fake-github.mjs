import { createServer } from "node:http";

const port = Number(process.env.GITHUB_TEST_PORT ?? "3101");
const issueStates = new Map();

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

  if (request.method === "GET" && url.pathname === "/search/issues") {
    sendJson(response, 200, {
      items: [
        {
          id: 94812,
          number: 4812,
          title: "Fix overlapping navigation controls",
          html_url: "https://github.com/LadybirdBrowser/ladybird/issues/4812",
          state: issueStates.get(4812) ?? "open",
          updated_at: "2026-09-15T12:00:00Z",
        },
        {
          id: 96200,
          number: 6200,
          title: "Investigate renderer overlap",
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
        : "Fix overlapping navigation controls",
      html_url: `https://github.com/LadybirdBrowser/ladybird/issues/${number}`,
      state: issueStates.get(number) ?? "open",
      updated_at: new Date().toISOString(),
    });
    return;
  }

  if (request.method === "PATCH" && issueMatch) {
    let body = "";
    request.setEncoding("utf8");
    request.on("data", (chunk) => {
      body += chunk;
    });
    request.on("end", () => {
      const number = Number(issueMatch[1]);
      const state = JSON.parse(body).state;
      if (state !== "open" && state !== "closed") {
        sendJson(response, 422, { message: "Invalid state" });
        return;
      }

      issueStates.set(number, state);
      sendJson(response, 200, {
        id: number + 90000,
        number,
        title: number === 6200
          ? "Investigate renderer overlap"
          : "Fix overlapping navigation controls",
        html_url: `https://github.com/LadybirdBrowser/ladybird/issues/${number}`,
        state,
        updated_at: new Date().toISOString(),
      });
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
      issueStates.set(7300, "open");
      sendJson(response, 201, {
        id: 97300,
        number: 7300,
        title: issue.title,
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
