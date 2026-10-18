// Plex Mono, preloaded, for the routes that may read it from their first paint (the account pages and the stepper, whose
// Agent view is Mono throughout). `/` never imports this module: its statement is Sans, Mono waits for the hero's hand-over,
// and its LCP image does not share the slow pipe with two font files (Lighthouse mobile, 09:20: LCP 2.63 s -> 2.47 s).
// Same family, weights and variable as the root layout's instance; this one's faces load later in those routes, so they win.
import { IBM_Plex_Mono } from "next/font/google";

export const monoPreload = IBM_Plex_Mono({ subsets: ["latin"], weight: ["400", "500"], display: "swap", variable: "--font-plex-mono" });
