# DESIGN.md — Tameion

Purpose: the single source for every visual decision in `apps/home` (the landing, the Owner view, the Agent view). Every colour, type role, radius, space and motion value resolves to a token in `packages/ui-tokens/tokens.json`, generated from the tables below; a contrast script fails the build when any text pair drops under 4.5:1 in either view.

Decision D-HOME-1, founder, 2026-10-18: **"The strait becomes the book."** The home page opens on the Strait of Hormuz seen from orbit. As you scroll, the photograph is quantised to a grid, drained of colour, its vessels tracked and labelled, and the labels become the rows of a real account. One account, two readers: the Owner stands on land (paper); the Agent is at sea (dark). One toggle.

## Why this and not a dashboard
- Trade moves before money does. A strait is where physical trade becomes obligation at scale: bills of lading, dues, demurrage, bunker invoices. Agents have a strait too, the gateway their calls and payments pass through. Tameion is the book on both sides, in dollars that hold still.
- The memorable thing is one mechanism, not decoration: **the pixel grid becomes the ledger grid.** Quantisation is what tracking does to a photograph; a ledger is what a treasury does to trade. Colour leaves as the image becomes a record; what remains is ink on paper.
- Nothing on screen is invented: the photograph is a dated Sentinel-2 scene, the vessels are a recorded AIS window with real MMSIs, the rows are a real (demo) account's records. Proof, not mood.

## Direction in one line
Ledger paper, black ink, the strait's own dark water for the agent, one red for the moments that need a person, and figures that cannot be misread.

## Palette
| Token | Light · Owner · "land" | Dark · Agent · "sea" | Used for |
|---|---|---|---|
| paper | `#EEF2EA` | `#0E1B22` | the page |
| plate | `#FFFFFF` | `#15242C` | a group on the page (level 1) |
| plate-2 | `#E4EAE1` | `#1D2F38` | a group inside a sheet, inputs, pressed rows |
| ink | `#000000` | `#EEF2EA` | text 1, the one primary action |
| ink-2 | `#5B625B` | `#A9B4AD` | secondary text (≥ 4.5:1 in both) |
| rule | `#D5DCD2` | `#2A3C45` | the ledger's ruling: table rows only, never a section divider |
| attention | `#B3261E` | `#FF6B5E` | Needs you, Stop, refused, failures. Nothing else |
| transit | `#3FB8B0` | `#3FB8B0` | marks only: reserved, in flight, a vessel under way. Never text on light |
| sealed | ink | ink | the symbolon mark when all three halves are present: drawn, not coloured |
| up / down | `#2F7A00` / `#B3261E` | `#9FD66A` / `#FF6B5E` | direction only, always with a sign or a word |
| scrim / skeleton | `#000000B3` / `#DEE4DA` | `#000000B3` / `#22343D` | sheets and loading geometry |

The dark values are taken from the photograph's deep water, the transit mark from its shallows. Testnet is a word and a shape (a dashed capsule), never a colour wash. Real marks (USDC, Arc, vendors) keep their own colours; nothing is tinted.

## Typography
- **IBM Plex Sans** 400 / 500 / 600 for everything a person reads. **IBM Plex Mono** 400 / 500 for everything the machine's eye produces: the hero's tracking labels and the whole Agent view. One superfamily, two roles; the change of face is the change of reader.
- Tabular lining figures wherever money or time appears (`font-variant-numeric: tabular-nums lining-nums`).
- Scale, base 16, ratio 1.25: 13 · 16 · 20 · 25 · 31 · 39 · 49 · 61.
  Roles: heroStatement 49/56 (31/38 under 640 px) · authority 31/38 · balance 39/44 · sheetTitle 25/32 · rowTitle 16/24 at 600 · row 16/24 · meta 13/18 · monoDense 13/20 · monoRead 15/22. Tracking 0; heroStatement −0.01 em. Measure ≤ 68 ch. Sentence case.
- Banned: tracked uppercase, eyebrow labels above headings, one accented word inside a headline, monospace anywhere in the Owner view.

## Shape, space, elevation
- Radius: 4 (marks) · 8 (inner plates, inputs) · 16 (plates) · 24 (sheet top). Buttons 52 high (44 compact), radius 12. Touch targets ≥ 44.
- Space on a 4-grid: 4 · 8 · 12 · 16 · 24 · 32 · 48 · 64. Gutter 20 (mobile) / 32 (desktop). Content max 1120.
- Depth is one step of fill, never a border or a shadow. Shadows only on a sheet (`0 -4 16 #0000001F`) and on the mode capsule while it slides.

## Motion
- One orchestrated moment: the hero sequence below, bound to scroll, not to time. Everything else answers a tap: press 100 ms in / 160 ms out to 0.98; the capsule crossfades in 200 ms; live figures roll in 160 ms; a sheet 320 ms in / 200 ms out on `(0.32, 0.72, 0, 1)`.
- Reduced motion: the hero shows three stills (photograph, grid, book); the capsule swaps instantly; figures swap.
- No entrance animations on sections, no hover choreography on cards, no parallax outside the hero.

## The hero: six keyframes bound to scroll progress p
| p | The canvas | The words |
|---|---|---|
| 0.00 | Sentinel-2 true-colour scene of the strait, full bleed, natural colour. A paper plate at the left holds the statement and two actions. Caption bottom-right: "Strait of Hormuz · Sentinel-2 · <scene date>". | **Trade moves before money does.** Every ship through this strait is a promise someone has to keep. Tameion is the book your agents keep, in dollars that hold still. [Open the book] [Try it in 5 minutes] |
| 0.20 | Quantisation: the image is resampled to a square grid whose cell grows from 2 px to the ledger module (row height 40 px; columns in multiples of 8 px), nearest-neighbour upscaling. Colour intact. | unchanged |
| 0.40 | Drain: saturation to zero, levels to two tones. Land becomes paper; sea becomes ruled paper (horizontal rules at the row module); the vessels of the AIS replay remain as ink clusters. | The sea becomes a page. |
| 0.60 | Tracking: each cluster gets a thin underline and a Plex Mono label from the recording: `MMSI 4…  tanker  11.8 kn  06:41`. Labels align to the grid's columns. No boxes, no brackets, no crosshairs. | Every vessel is seen, named and timed. |
| 0.80 | Hand-over: the vessel labels slide into table rows and fade; the real records of the demo account take their place in the same columns: date · who · amount · why · state. | Your agents pass through a strait too. Every call, every bill, every refusal: one book. |
| 1.00 | The Owner home of the demo account, live, on paper, zero hashes. The header capsule reads Owner \| Agent. | the product |

Data and honesty for the hero
- Imagery: Copernicus Sentinel-2 L2A true colour, bbox 54.5–58.0 E × 24.5–27.5 N, exported from the Copernicus Browser at ≤ 2560 px wide, AVIF ≤ 350 KB, with the scene date and "Contains modified Copernicus Sentinel data (2026)". Never a frame from a video; never ShipXplorer or MarineTraffic tiles.
- Vessels: an aisstream.io recording for the same bbox (position reports: MMSI, lat, lon, SOG, COG, ship type, timestamp); the window is stated in the caption; replayed by scroll and projected with the image's world file. Labels show only what the recording holds: no invented names, no invented cargo, no money attributed to any ship.
- Rows: the demo account's real records, labelled "demo account" in the caption. The hero never shows a ship's finances; it hands over to ours.
- Coastline (treatment B): Natural Earth 10 m, public domain.

## Page grammar
- One hero per surface. On the Owner home the hero is the authority sentence, not a number. At most four facts at rest. Detail behind one disclosure. No explainer sentence under a control.
- Left-aligned throughout. Text never sits over the strait's bend (the canvas's focal third); it sits on the paper plate.
- Inputs and the action fit 375 × 812; the action is a 52 pt plate above the keyboard. Sheets open at content size, or at 0.62 for lists; on desktop, a 400 px right drawer.
- The Owner view contains no hash: a test fails on `/0x[0-9a-fA-F]{8,}/` anywhere in its DOM. The Agent view is where hashes live.

## States, designed on every surface
Loading: a skeleton shaped like the layout, held ≥ 300 ms, never a spinner page, never a fake $0. Empty: says why and names the next action. Error: says what still works, plus one action; technical text behind a disclosure. Offline: cached values at full ink, marked stale with their time. Waits: a short verb ("Paying", "Checking"), elapsed seconds after 3 s, "Still checking. It will not be sent twice." after 15 s.

## Money lifecycle
A blocked action names its fix and does it ("Add money"). In flight the action names the phase, Signing → Sent → Confirmed, and never looks disabled; it morphs into the record. A failure card always says whether money moved. A lost answer is "still checking" and is never re-sent.

## Copy
Active voice. The action says what happens ("Stop the agent" → "Stopped"). The same word through the whole flow. Plain words: allowance, paid, refused, needs you, sealed. Never "Submit", never "Soon", never "Quote only". The authority receipt has three sections, "Tameion can", "Tameion can never", "Worst case", each generated from the mandate, nothing hand-written.

## Rules
- No hex outside the tokens file. No hard-coded strings in components.
- Attention red only for Needs you, Stop, refused and failures. Transit only as a mark. Up and down only for direction, never alone.
- Mono only for the machine's eye. Hashes only in the Agent view.
- Real marks for every token, chain and vendor; generic glyphs are utility only.
- Motion on wrappers, colour on inner views. Reduced motion respected everywhere.

## Banned
HUD chrome (brackets, crosshairs, scanlines, radar sweeps, glow); gradients; borders on content; shadows on groups; tracked uppercase; eyebrow labels; meta strings joined with middle dots; "→" in links or buttons; cream paper with a serif and a clay accent; neon on black; a spinner page; "Soon"; a placeholder figure; tinted logos; stock photographs of containers or handshakes; any vessel, route, scene or number that was not recorded.

## Three treatments to preview before the values are frozen (same concept, same token names)
- **A. Photograph → book** (default; the storyboard above).
- **B. Chart → book**: no photograph. An ink-on-paper line chart of the strait from Natural Earth, vessels as marks; the drain is chart → table. Lighter, loads instantly, reads less like a news image.
- **C. Numbers → book**: no map. The day's transits (IMF PortWatch daily count for the strait, beside our AIS window) typeset at 61 px, re-sorting into the account's rows. The boldest and the cheapest.
Screenshots at 1280 and 390, both views. The founder chooses; only token values change, no component is rebuilt.

## Acceptance: what shows it is right, other than us
- Five-second test: five people who have not seen the product look at the hero for five seconds and answer "what does this company do?". Four of five say finance, a ledger, an account for agents; none says an oil or shipping company.
- Lighthouse mobile performance ≥ 85 with the hero; LCP ≤ 2.5 s; the canvas starts after LCP; the hero image ≤ 350 KB.
- axe-core clean in both views; reduced motion verified by screenshot.
- In CI: the zero-hash test, the tokens-only hex lint, the contrast script, `21st review` with zero errors.
- The recording file and the scene id live in `proof/home-hero-<date>/`; any MMSI shown can be looked up on a public AIS site for that hour.
- The judges' live path (the account page) loads with no hero and is unchanged.
