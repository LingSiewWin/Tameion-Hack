// Every word onboarding shows a person (DESIGN Rules: no hard-coded strings in components). Sentence case, active voice,
// the same word through the whole flow. The receipt's lines are built from the mandate in receipt.ts, not here.

export const guest = {
  title: "Open the book",
  word: "The word from the judges' form",
  demo: "Open the demo account",
  own: "Make your own",
  noWord: "This host has no invite word set, so it opens nothing yet.",
  wrongWord: "That is not the word from the judges' form.",
  noDemo: "The word is right, but this host names no demo account. Make your own instead.",
};

export const steps = ["Name the account", "Your passkey", "Funded", "Allowance"] as const;
export const stepOf = (i: number, n: number, label: string) => `Step ${i + 1} of ${n}: ${label}`;

export const load = {
  down: "The service did not answer, so no account can open yet.",
  retry: "Try again",
  demoOff: "This host does not open demo accounts. The demo account on the first page still opens.",
};

export const name = {
  title: "Name the account",
  invalid: "3 to 32 lowercase letters, numbers or dashes, not starting or ending with a dash.",
  checking: (n: string) => `Checking ${n}.`,
  unknown: "The name could not be checked just now; opening still tells you.",
  taken: (n: string) => `${n} is taken. Try another.`,
  free: (n: string) => `${n} is free.`,
  go: "Continue",
  fix: "Try another name",
};

export const passkey = {
  title: "Your passkey",
  // The brief's line said "the only key that can change the rules"; the operator key may also propose a raise
  // (BudgetPolicy.setLimits is onlyPrincipalOrOperator), so the line names what only the passkey can do.
  primer: "Your face or fingerprint is the only key that can approve a payee or move the money. We never have it.",
  make: "Make my passkey",
  use: "Use my passkey",
  waiting: "Waiting for your passkey",
  cancel: "Cancel",
};

export const fund = {
  title: (chain: string) => `Funded on ${chain}`,
  stops: ["Opened", "Sent", "Arrived"] as const,
  label: (stop: string) => `Funding: ${stop}`,
  opening: (n: string, chain: string) => `Opening ${n} on ${chain}.`,
  sent: (amount: string) => `Sent ${amount} to the account; reading the balance back.`,
  arrived: (amount: string) => `The account holds ${amount}.`,
  demoAmount: "the demo's USDC",
  failedTitle: "Nothing was sent.",
  slowTitle: "Sent; the balance is still being read.",
  notFunded: "The demo did not fund the account.",
  slow: "The funding was sent, but the balance has not read back after two minutes.",
  retry: "Try again",
};

export const allow = {
  title: "Allowance",
  vendor: "Demo Vendor",
  go: (amount: string, every: string) => `Let the agent pay Demo Vendor ${amount} every ${every}`,
  phases: { Signing: "Signing", Sent: "Sent", Confirmed: "Confirmed" },
  lostTitle: "Still checking: the approval may have gone through. Nothing was paid.",
  approvedTitle: "Demo Vendor is approved; nothing was paid.",
  failedTitle: "Nothing was sent.",
  challengeDiffers: "Not signing: the service's challenge differs from the one this page computed.",
  moreOwners: "Approved by you; another owner key must approve too, on its own device.",
  agentKeys: "This account has agent keys, so its bills are saved from the account page with the passkey.",
  notReadBack: "Demo Vendor is approved and the bill is saved, but the account has not read them back yet.",
};

export const wait = {
  seconds: (n: number) => `${n} s`,
  still: "Still checking. It will not be sent twice.",
  details: "What the service said",
};

export const receipt = {
  title: (n: string) => `${n} is open.`,
  holds: (amount: string, chain: string) => `It holds ${amount} on ${chain}.`,
  sections: { can: "Tameion can", never: "Tameion can never", worst: "Worst case" },
  open: "Open the account",
};

export const receive = {
  title: "Receive",
  qr: (handle: string) => `QR code of ${handle}`,
  arrives: (chain: string) => `Any USDC on ${chain} arrives here.`,
  down: "The service did not answer, so the address cannot be shown. Reload the page to try again.",
  unknown: (n: string) => `No account has the name ${n}. Check the name in the link you were sent.`,
  copy: "Copy",
  copied: "Copied",
  share: "Share",
};

export const server = {
  noInvite: "Type the word from the judges' form on the first page, then open your account.",
  badName: "Pick a name: 3 to 32 lowercase letters, numbers or dashes, not starting or ending with a dash.",
  badKey: "The passkey's public key did not come through; make the passkey again.",
  down: "The service did not answer.",
  downOpen: "The service did not answer. Nothing was opened; try again.",
  refused: (status: number) => `The service refused (${status}).`,
  notOwnerRoute: "Not an owner route of this page.",
  notAddress: "Not an account address.",
};
