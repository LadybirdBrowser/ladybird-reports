import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";

export default function globalSetup(): void {
  const sessionToken = execFileSync(
    "cargo",
    ["run", "--quiet", "--bin", "browser_test_fixture"],
    {
      env: {
        ...process.env,
        BROWSER_TEST_FIXTURE: "enabled",
      },
      encoding: "utf8",
      stdio: ["ignore", "pipe", "inherit"],
    },
  ).trim();

  mkdirSync("target", { recursive: true });
  writeFileSync(
    "target/browser-test-storage-state.json",
    JSON.stringify({
      cookies: [
        {
          name: "session",
          value: sessionToken,
          domain: "127.0.0.1",
          path: "/",
          httpOnly: true,
          secure: false,
          sameSite: "Lax",
          expires: -1,
        },
      ],
      origins: [],
    }),
    { mode: 0o600 },
  );
}
