import { test, expect } from "@playwright/test";

const USER = process.env.ADMIN_E2E_USER || "admin";
const PASS = process.env.ADMIN_E2E_PASS || "test-admin-pass";

async function login(page: import("@playwright/test").Page) {
  await page.goto("/admin/");
  await expect(page.getByRole("heading", { name: "tailsvc admin" })).toBeVisible();
  await page.getByLabel("Username").fill(USER);
  await page.getByLabel("Password").fill(PASS);
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page.getByRole("button", { name: "Refresh" })).toBeVisible({
    timeout: 15_000,
  });
  await expect(page.getByRole("button", { name: "Log out" })).toBeVisible();
}

test.describe("admin panel", () => {
  test("login page rejects bad password", async ({ page }) => {
    await page.goto("/admin/");
    await page.getByLabel("Username").fill(USER);
    await page.getByLabel("Password").fill("wrong-password");
    await page.getByRole("button", { name: "Sign in" }).click();
    await expect(page.locator("#login-error")).toContainText(/Invalid|Unauthorized|failed/i, {
      timeout: 10_000,
    });
    await expect(page.getByRole("button", { name: "Sign in" })).toBeVisible();
  });

  test("login succeeds and shows dashboard chrome", async ({ page }) => {
    await login(page);
    await expect(page.locator("#c-status")).not.toHaveText("—", { timeout: 20_000 });
    await expect(page.locator("#c-routes")).toBeVisible();
    await expect(page.locator("#c-agents")).toBeVisible();
    await expect(page.locator("#c-disc")).toBeVisible();
    await expect(page.locator("#c-probe-ok")).toBeVisible();
    await expect(page.getByRole("heading", { name: /Discovery/i })).toBeVisible();
    await expect(page.getByRole("heading", { name: "Registered sites" })).toBeVisible();
    await expect(page.getByRole("heading", { name: "Agents" })).toBeVisible();
  });

  test("refresh button reloads dashboard without error", async ({ page }) => {
    await login(page);
    await page.getByRole("button", { name: "Refresh" }).click();
    await expect(page.locator("#error")).toBeHidden({ timeout: 15_000 });
    await expect(page.locator("#c-status")).not.toHaveText("—");
  });

  test("new enrollment token button shows token", async ({ page }) => {
    await login(page);
    const btn = page.getByRole("button", { name: "New enrollment token" });
    await expect(btn).toBeVisible();
    await btn.click();
    // Button may briefly show Creating…
    await expect(page.locator("#enroll-section")).toBeVisible({ timeout: 20_000 });
    const token = page.locator("#enroll-token");
    await expect(token).toBeVisible();
    const text = (await token.textContent())?.trim() || "";
    expect(text.length).toBeGreaterThan(20);
    await expect(page.locator("#error")).toBeHidden();
  });

  test("logout returns to login form", async ({ page }) => {
    await login(page);
    await page.getByRole("button", { name: "Log out" }).click();
    await expect(page.getByRole("button", { name: "Sign in" })).toBeVisible({
      timeout: 10_000,
    });
    await expect(page.getByLabel("Password")).toBeVisible();
  });

  test("dashboard API unauthorized without session", async ({ request, baseURL }) => {
    const res = await request.get(`${baseURL}/v1/admin/dashboard`);
    expect(res.status()).toBe(401);
  });

  test("login API issues bearer then dashboard works", async ({ request, baseURL }) => {
    const login = await request.post(`${baseURL}/v1/admin/login`, {
      data: { username: USER, password: PASS },
    });
    expect(login.ok()).toBeTruthy();
    const { token } = await login.json();
    expect(token).toBeTruthy();

    const dash = await request.get(`${baseURL}/v1/admin/dashboard`, {
      headers: { Authorization: `Bearer ${token}` },
    });
    expect(dash.ok()).toBeTruthy();
    const body = await dash.json();
    expect(Array.isArray(body.routes)).toBeTruthy();
    expect(Array.isArray(body.agents)).toBeTruthy();

    const disc = await request.get(`${baseURL}/v1/admin/discovery`, {
      headers: { Authorization: `Bearer ${token}` },
    });
    expect(disc.ok()).toBeTruthy();
    expect(Array.isArray(await disc.json())).toBeTruthy();

    const enroll = await request.post(`${baseURL}/v1/admin/enrollment-tokens`, {
      headers: { Authorization: `Bearer ${token}` },
      data: {},
    });
    expect(enroll.ok()).toBeTruthy();
    const en = await enroll.json();
    expect(en.token.length).toBeGreaterThan(20);

    await request.post(`${baseURL}/v1/admin/logout`, {
      headers: { Authorization: `Bearer ${token}` },
    });
  });

  test("required DOM ids exist in HTML", async ({ page }) => {
    await page.goto("/admin/");
    const html = await page.content();
    for (const id of [
      "c-status",
      "c-routes",
      "c-agents",
      "c-disc",
      "c-probe-ok",
      "disc-body",
      "routes-body",
      "agents-body",
      "whoami",
      "login-view",
      "app-view",
      "enroll-section",
      "enroll-token",
      "btn-enroll",
      "btn-refresh",
      "btn-logout",
      "btn-login",
    ]) {
      expect(html, `missing #${id}`).toContain(`id="${id}"`);
    }
  });
});
