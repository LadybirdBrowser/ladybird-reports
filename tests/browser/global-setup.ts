import { execFileSync } from "node:child_process";

export default function globalSetup(): void {
  execFileSync("cargo", ["run", "--quiet", "--bin", "browser_test_fixture"], {
    env: {
      ...process.env,
      BROWSER_TEST_FIXTURE: "enabled",
    },
    stdio: "inherit",
  });
}
