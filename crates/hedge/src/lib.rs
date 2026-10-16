//! Keeps the treasury delta-neutral and books every step into [`ledger`].
//!
//! The treasury holds volatile assets (ETH, BTC, SOL) and owes USDC. This
//! crate opens the offsetting perpetual short on Hyperliquid, keeps it inside
//! a band as prices and holdings move, refuses any order that would leave the
//! margin account inside the venue's liquidation waterfall, and records every
//! action as a balanced double-entry line with its reason. Spec: `docs/43`.
//! Safety-engine evidence: `docs/42`.
//!
//! Layout, and what talks to what:
//!
//! - [`policy`], [`exposure`], [`delta`], [`safety`], [`book`]: pure. No I/O,
//!   no async, no venue types. Everything a reviewer has to trust about *what
//!   the hedge may do* lives here and is exhaustively testable.
//! - [`hl`]: the one module that talks to Hyperliquid, through `hypersdk`.
//!   It reads markets, prices, margin and positions, and places orders. It
//!   converts venue types into the pure types above at the boundary.
//! - [`engine`]: one step of the loop. Read, decide, (maybe) trade, read
//!   back, book. Writes `HALT` and stops on any mismatch.
//! - [`halt`]: the halt file.
//! - [`store`]: the books on disk, append-only, re-posted on load.
//! - [`record`]: `EventBody::Hedge` for the anchored log, JSON for the page.
//! - [`driver`]: environment → one step; shared by the demo server and the
//!   chain examples so they wire the hedge in identically.
//!
//! Three rules that are not negotiable (see `CLAUDE.md`):
//! the hedge is deterministic policy, never a model's call; margin is plain
//! USDC; no order is "done" until the position is read back and matches.

pub mod book;
pub mod delta;
pub mod driver;
pub mod engine;
pub mod exposure;
pub mod halt;
pub mod hl;
pub mod policy;
pub mod record;
pub mod safety;
pub mod store;

pub use rust_decimal::{Decimal, dec};
