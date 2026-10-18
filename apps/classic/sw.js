// The account page's service worker (docs/65): shows what the agent pushes
// after a check (operator::push), and opens the account when tapped.
self.addEventListener("push", (e) => {
  let m = { title: "Tami", body: "" };
  try { m = e.data ? e.data.json() : m; } catch { m.body = e.data ? e.data.text() : ""; }
  e.waitUntil(self.registration.showNotification(m.title || "Tami", { body: m.body, icon: "/icon.svg", data: { policy: m.policy } }));
});
self.addEventListener("notificationclick", (e) => {
  e.notification.close();
  const p = e.notification.data && e.notification.data.policy;
  e.waitUntil(clients.openWindow(p ? `/?policy=${p}` : "/"));
});
