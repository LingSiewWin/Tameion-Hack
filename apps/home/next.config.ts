import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  // A cached build served the previous Tailwind output after a stylesheet edit (docs/60 P-48); tokens.css changes
  // often here, and the build is small, so build without the cache.
  experimental: { turbopackFileSystemCacheForBuild: false },
  // This app's own folder: a stray lockfile higher up otherwise becomes the workspace root.
  turbopack: { root: process.cwd() },
};

export default nextConfig;
