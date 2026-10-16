import type { NextConfig } from "next";

// The console is a static export the Rust service serves at `/` (same origin
// as `/api`, so passkeys and the API need no CORS). In `next dev` the export
// is off and the service is reached through rewrites — the two cannot be on
// together (next docs: static-exports, "Unsupported Features").
const dev = process.env.NODE_ENV === "development";
const service = process.env.TAMEION_SERVICE ?? "http://127.0.0.1:7880";

const nextConfig: NextConfig = {
  reactCompiler: true,
  // A cached build served the previous Tailwind output after globals.css changed
  // (docs/60 P-48); a static export is small, so build without the cache.
  experimental: { turbopackFileSystemCacheForBuild: false },
  images: { unoptimized: true },
  turbopack: {
    // This app's own folder: a stray lockfile in the home directory otherwise
    // becomes the workspace root.
    root: __dirname,
    rules: {
      "*.css": {
        loaders: ["@tailwindcss/turbopack"],
        as: "*.css",
      },
    },
  },
  ...(dev
    ? {
        // `next dev` gzips the proxied SSE and holds frames back, so /agent sat on "Connecting"
        // (docs/60 P-152); the service's own copy on :7880 is unaffected.
        compress: false,
        async rewrites() {
          return [
            { source: "/api/:path*", destination: `${service}/api/:path*` },
            { source: "/level1/:path*", destination: `${service}/level1/:path*` },
            { source: "/sw.js", destination: `${service}/sw.js` },
            { source: "/agents/:path*", destination: `${service}/agents/:path*` },
            { source: "/proof/:path*", destination: `${service}/proof/:path*` },
            { source: "/manifest.webmanifest", destination: `${service}/manifest.webmanifest` },
          ];
        },
      }
    : { output: "export" as const }),
};

export default nextConfig;
