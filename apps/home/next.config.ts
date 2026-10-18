import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  // A cached build served the previous Tailwind output after a stylesheet edit (docs/60 P-48); tokens.css changes
  // often here, and the build is small, so build without the cache.
  experimental: { turbopackFileSystemCacheForBuild: false },
  // This app's own folder: a stray lockfile higher up otherwise becomes the workspace root.
  turbopack: { root: process.cwd() },
  // The console's old pages (docs/122): with TAMEION_HOME_URL set the Rust service answers 302 to home for `/`, `/agent`,
  // `/pay` and `/watch`, path and query kept. `/agent?policy=` opens that account's Agent view (the front page turns the
  // address into the name); `/pay?to=<name>` was "what someone paying the account needs" (its address and chains), which
  // home's /a/<name>/receive shows; `/watch` (watch any address's bills before opening an account) has no home page yet.
  // Query strings pass through to the destination.
  redirects: async () => [
    { source: "/agent", has: [{ type: "query", key: "policy" }], destination: "/?view=agent", permanent: false },
    { source: "/agent", destination: "/", permanent: false },
    { source: "/pay", has: [{ type: "query", key: "to", value: "(?<to>[a-z0-9][a-z0-9-]{1,30}[a-z0-9])" }], destination: "/a/:to/receive", permanent: false },
    { source: "/pay", destination: "/", permanent: false },
    { source: "/watch", destination: "/", permanent: false },
  ],
};

export default nextConfig;
