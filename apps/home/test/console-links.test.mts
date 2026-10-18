// The console's old links keep working on home (docs/122, after M2's switch: with TAMEION_HOME_URL set the Rust service
// answers 302 to home for `/`, `/agent`, `/pay`, `/watch`, path and query kept). next.config sends `/agent?policy=` to the
// front page with `view=agent`, `/pay?to=<name>` to that account's receive page, the rest to `/`; the front page turns
// `?policy=<address>` into the account's name through the service's tenant route (no address ever reaches the route).
// Verified live against the dev server too (proof/retire-console-2026-10-18/home-console-links.txt).
import assert from "node:assert/strict";
import { test } from "node:test";
import { VIEW_BOOT, VIEW_KEY } from "../src/tokens.ts";

const ADDR = "0x8e388dc7a4098b0e7837b6607a70114238655623";

test("next.config: the console's /agent, /pay and /watch land on home's pages", async () => {
  const { default: config } = await import("../next.config.ts");
  const r = await config.redirects!();
  assert.deepEqual(
    r.map((x) => [x.source, x.has?.map((h) => `${h.type}:${h.key}${"value" in h && h.value ? `=${h.value}` : ""}`).join(",") ?? "", x.destination, x.permanent]),
    [
      ["/agent", "query:policy", "/?view=agent", false],
      ["/agent", "", "/", false],
      ["/pay", "query:to=(?<to>[a-z0-9][a-z0-9-]{1,30}[a-z0-9])", "/a/:to/receive", false],
      ["/pay", "", "/", false],
      ["/watch", "", "/", false],
    ],
  );
  const to = new RegExp(`^${(r[2].has![0] as { value: string }).value}$`);
  assert.ok(to.test("guest-demo-arc"));
  for (const bad of ["Guest", "-a", "a", "a/b", ADDR.slice(0, 3) + "/x"]) assert.ok(!to.test(bad), bad);
});

test("?policy=<address> becomes the account's page by its name; a name passes through; nothing else is a route", async () => {
  process.env.TAMEION_URL = "https://service.example";
  const asked: string[] = [];
  globalThis.fetch = (async (u: string) => {
    asked.push(String(u));
    return String(u).endsWith(`/api/level1/tenant/${ADDR}`) ? Response.json({ tenant: { policy: ADDR, name: "guest-demo-arc" } }) : Response.json({ error: "no such tenant" }, { status: 404 });
  }) as typeof fetch;
  const { accountHref } = await import("../src/lib/api.ts");
  assert.equal(await accountHref(ADDR), "/a/guest-demo-arc");
  assert.equal(await accountHref(ADDR, "agent"), "/a/guest-demo-arc?view=agent");
  assert.equal(asked[0], `https://service.example/api/level1/tenant/${ADDR}`, "asked on the server, of the service itself");
  assert.equal(await accountHref("0x" + "11".repeat(20)), null, "no account: the front page");
  assert.equal(await accountHref("Guest-Demo-Arc"), "/a/guest-demo-arc", "a name is a name");
  assert.equal(await accountHref("../x"), null);
});

test("?view=agent opens the Agent view (remembered), and blocked storage still leaves Owner", () => {
  const boot = (search: string, store: Record<string, string> | null) => {
    const html = { dataset: {} as Record<string, string> };
    const localStorage = store
      ? { getItem: (k: string) => store[k] ?? null, setItem: (k: string, v: string) => void (store[k] = v) }
      : {
          getItem: () => {
            throw new Error("SecurityError");
          },
          setItem: () => {
            throw new Error("SecurityError");
          },
        };
    new Function("localStorage", "document", "location", VIEW_BOOT)(localStorage, { documentElement: html }, { search });
    return html.dataset.view;
  };
  const store: Record<string, string> = {};
  assert.equal(boot("?view=agent", store), "agent");
  assert.equal(store[VIEW_KEY], "agent", "remembered for the next visit");
  assert.equal(boot("?view=owner", store), undefined);
  assert.equal(boot("", { [VIEW_KEY]: "agent" }), "agent", "a remembered choice still holds");
  assert.equal(boot("?view=evil", {}), undefined);
  assert.equal(boot("?view=agent", null), undefined);
});
