// The add-money plates' shared classes (tokens only, DESIGN Rule 2) and links.
import type { CctpChain } from "./cctp.ts";

export const FAUCET = "https://faucet.circle.com";
/** A group on the page: one step of fill above paper, 16 px radius, no border, no shadow (DESIGN §Shape). */
export const plate = "flex flex-col gap-(--space-4) rounded-(--radius-plate) bg-(--plate) p-(--space-5)";
/** A group inside a plate: one more step of fill. */
export const inner = "rounded-(--radius-inner) bg-(--plate-2) p-(--space-3)";
/** A secondary control: compact 44, radius 12, filled one step, no border. */
export const quiet = "min-h-(--button-compact) rounded-(--radius-button) bg-(--plate-2) px-(--space-4) text-(--ink) cursor-pointer";
/** The one primary action: 52 high, full width, ink (DESIGN §Shape, Palette). */
export const cta = "h-(--button) w-full rounded-(--radius-button) bg-(--ink) px-(--space-4) text-(--paper) row-title cursor-pointer";
export const link = "text-(--ink) underline underline-offset-2";
/** A link that stands on its own line or beside a 44 px control: a 44 px hit area by padding, the text unchanged (DESIGN
 *  touch targets; the Agent view's row links do the same, 46600dc). Every link on this page is one of these. */
export const rowLink = `${link} inline-flex min-h-(--touch) items-center`;
/** A step's mark (radius 4): done is drawn in ink (sealed), in flight is transit, next is one step of fill, failed is attention. */
export const mark = { done: "bg-(--sealed)", transit: "bg-(--transit)", next: "bg-(--plate-2)", failed: "bg-(--attention)" } as const;
/** An explorer page: the Agent view only (DESIGN Rule 6). */
export const explorer = (c: CctpChain, kind: "tx" | "address", id: string) => `${c.chain.blockExplorers?.default.url}/${kind}/${id}`;
