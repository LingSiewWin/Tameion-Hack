// The add-money plates' shared classes (tokens only, DESIGN Rule 2) and links.
import type { CctpChain } from "./cctp.ts";

export const FAUCET = "https://faucet.circle.com";
/** A plate: flat on paper, a 1 px hairline, 8 px radius. */
export const plate = "flex flex-col gap-(--space-3) rounded-(--radius-plate) border border-(--rule) bg-(--plate) p-(--space-4)";
/** A secondary control: 44 px, hairline, no fill. */
export const quiet = "min-h-(--touch) rounded-(--radius-plate) border border-(--rule) bg-transparent px-(--space-4) text-(--ink) cursor-pointer";
/** The CTA: a 56 px plate, full width, ink border (DESIGN §Shape). */
export const cta = "h-(--cta) w-full rounded-(--radius-plate) border border-(--ink) bg-(--plate) px-(--space-4) text-(--ink) row-title cursor-pointer";
export const link = "text-(--accent) underline underline-offset-2";
/** An explorer page: the Agent view only (DESIGN Rule 6). */
export const explorer = (c: CctpChain, kind: "tx" | "address", id: string) => `${c.chain.blockExplorers?.default.url}/${kind}/${id}`;
