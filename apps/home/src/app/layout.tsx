import type { Metadata, Viewport } from "next";
import { IBM_Plex_Mono, IBM_Plex_Sans } from "next/font/google";
import { Shell } from "../components/shell.tsx";
import { VIEW_BOOT } from "../tokens.ts";
import "./globals.css";

const sans = IBM_Plex_Sans({ subsets: ["latin"], weight: ["400", "500", "600"], display: "swap", variable: "--font-plex-sans" });
// Not preloaded here: `/` sets its statement in Sans and needs Mono only at the hero's hand-over; the routes that read Mono
// from the start (the account pages, the stepper) preload it in their layouts (src/app/mono-preload.ts).
const mono = IBM_Plex_Mono({ subsets: ["latin"], weight: ["400", "500"], display: "swap", variable: "--font-plex-mono", preload: false });

export const metadata: Metadata = {
  title: "Tameion",
  description: "Your agents pay their bills in USDC, up to the mandate you signed.",
};
export const viewport: Viewport = { width: "device-width", initialScale: 1 };

export default function RootLayout({ children }: Readonly<{ children: React.ReactNode }>) {
  return (
    // data-view is set before paint by VIEW_BOOT, so React's server markup (owner) may differ from the attribute
    <html lang="en" data-view="owner" className={`${sans.variable} ${mono.variable}`} suppressHydrationWarning>
      <head>
        <script dangerouslySetInnerHTML={{ __html: VIEW_BOOT }} />
      </head>
      <body>
        {/* the console's origin, read here on the server and handed down: no client code reads the environment */}
        <Shell consoleUrl={process.env.NEXT_PUBLIC_CONSOLE_URL ?? null}>{children}</Shell>
      </body>
    </html>
  );
}
