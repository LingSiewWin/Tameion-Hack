// The few shapes onboarding repeats, in one place so the token pass touches one file. Depth is one step of fill, never a
// border or a shadow (DESIGN §Shape): a plate on paper, inputs and inner groups on plate-2, the one primary action in ink.
// A plain module (no "use client"), so server components (Receive) read the same strings as client ones.

/** Type roles (DESIGN §Typography), as the classes tokens.css defines. */
export const T = {
  hero: "authority",
  title: "sheet-title",
  rowTitle: "row-title",
  row: "row",
  meta: "meta",
  money: "money",
  mono: "mono-dense",
  attention: "row text-(--attention)",
};

// Press: 100 ms in to 0.98 (DESIGN §Motion), on the wrapper; reduced motion keeps it still.
const BUTTON = "flex h-(--button) w-full items-center justify-center rounded-(--radius-button) px-(--space-4) text-center transition-transform duration-(--press-out) active:scale-(--press-scale) active:duration-(--press-in) motion-reduce:transition-none motion-reduce:active:scale-100";
/** The one primary action: ink. */
export const CTA = `${T.rowTitle} ${BUTTON} bg-(--ink) text-(--paper)`;
/** A second action: one step of fill. */
export const CTA_QUIET = `${T.rowTitle} ${BUTTON} bg-(--plate-2) text-(--ink)`;
export const FIELD = `${T.row} h-(--button) w-full rounded-(--radius-inner) bg-(--plate-2) px-(--space-3) text-(--ink)`;
export const PLATE = "rounded-(--radius-plate) bg-(--plate) p-(--space-5)";
