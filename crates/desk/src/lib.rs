//! The desk: a small trader's physical book, kept in double entry in the
//! unit of the goods, and what that book implies for the offset on the venue
//! and for a lender (`docs/83`).
//!
//! A large trading house books every purchase and sale as a position, offsets
//! the price while it holds the goods, and borrows against what is offset. A
//! lender reads that as a borrowing-base certificate: what is held, where,
//! which part counts (stock in a vault or in transit), which part does not
//! (work in fabrication, goods not yet delivered), what is consigned out, and
//! how much of it is hedged (`docs/84` §6: A-Mark advances 90 % on hedged
//! metal in a depository, 85 % unhedged, 70 % consigned). This crate is that
//! book for one trader:
//!
//! - a **milestone** is the only input: bought (a priced purchase), shipped,
//!   arrived (into stock or a vault), to fabrication, consigned, returned,
//!   sold (a priced sale). Each names its document, its counterparty and,
//!   when it came signed, the address that signed it;
//! - each milestone posts balanced [`Entry`]s in grams (goods) and in USDC
//!   (cost, proceeds), so the stage balances *are* the exposure;
//! - [`Desk::offset_target`] turns the exposure above the trader's own
//!   unhedged base (stock they replace every day anyway, `docs/84` §1) into a
//!   size on the venue; [`Desk::statement`] into what a lender reads;
//!   [`Desk::disclose`] into one deal's milestones with Merkle paths to the
//!   root of all of them.
//!
//! Pure, like `ledger`: no I/O, no clock, no venue. Prices and the venue's
//! position come in as arguments.

use core::fmt;
use std::collections::BTreeMap;

use ledger::log::{
    Hash32, ProofStep, consistency_from_leaves, leaf_from_canonical, path_from_leaves,
    root_from_leaves,
};
use ledger::{
    Account, AccountId, AccountKind, BalanceError, Chart, Credit, Currency, Debit, DocumentKind,
    DocumentRef, Entry, Journal, Money, MoneyError, PostError,
};
use rust_decimal::{Decimal, RoundingStrategy, dec};
use serde::{Deserialize, Serialize};

/// The largest step the desk books: 100 tonnes, in milligrams.
const MAX_STEP_MINOR: i128 = 100_000_000_000;
/// The largest price on one contract: ten billion USDC, in minor units.
const MAX_USDC_MINOR: i128 = 10_000_000_000_000_000;

/// Goods the desk can book: one variant per good with a market that offsets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Goods {
    Gold,
}

impl Goods {
    /// The unit the trader books in.
    pub const fn unit(self) -> Currency {
        match self {
            Goods::Gold => Currency::GoldGram,
        }
    }

    /// The Hyperliquid market that offsets it. PAXG is a core perp (validator
    /// operated, liquidator-vault backstop); one PAXG is one fine troy ounce.
    pub const fn market(self) -> &'static str {
        match self {
            Goods::Gold => "PAXG",
        }
    }

    /// Book units in one venue unit: grams in a troy ounce, exactly.
    pub fn units_per_contract(self) -> Decimal {
        match self {
            Goods::Gold => dec!(31.1034768),
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Goods::Gold => "gold",
        }
    }

    pub fn parse(s: &str) -> Option<Goods> {
        match s {
            "gold" => Some(Goods::Gold),
            _ => None,
        }
    }
}

/// Where goods sit. Each stage is one account per deal, in the goods' unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Priced and owed to us, not yet delivered.
    Bought,
    InTransit,
    /// In our stock: a vault, a safe, a warehouse.
    InStock,
    /// With a fabricator or in the workshop: work in process.
    InFabrication,
    /// Placed with a retailer, still ours until they sell it.
    Consigned,
}

impl Stage {
    pub const ALL: [Stage; 5] = [
        Stage::Bought,
        Stage::InTransit,
        Stage::InStock,
        Stage::InFabrication,
        Stage::Consigned,
    ];

    pub const fn words(self) -> &'static str {
        match self {
            Stage::Bought => "bought, not delivered",
            Stage::InTransit => "in transit",
            Stage::InStock => "in stock",
            Stage::InFabrication => "in fabrication",
            Stage::Consigned => "consigned",
        }
    }

    const fn account(self) -> &'static str {
        match self {
            Stage::Bought => "goods:bought",
            Stage::InTransit => "goods:in-transit",
            Stage::InStock => "goods:in-stock",
            Stage::InFabrication => "goods:in-fabrication",
            Stage::Consigned => "goods:consigned",
        }
    }

    /// How a lender classes it (`docs/84` §6, the OCC handbook and A-Mark's
    /// credit agreement): stock and insured transit count; work in process
    /// and goods not yet delivered do not; consigned goods are their own
    /// class with their own advance rate.
    pub const fn class(self) -> Class {
        match self {
            Stage::InStock | Stage::InTransit => Class::Eligible,
            Stage::Consigned => Class::Consigned,
            Stage::Bought | Stage::InFabrication => Class::Ineligible,
        }
    }
}

/// A lender's class for a stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Class {
    Eligible,
    Consigned,
    Ineligible,
}

/// What happened to a deal. Contracts (bought, sold) carry a USDC amount;
/// movements carry only goods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MilestoneKind {
    Bought,
    /// Bought → in transit (bill of lading, air waybill, courier receipt).
    Shipped,
    /// In transit, or straight from the supplier, → in stock (vault receipt).
    Arrived,
    /// In stock → in fabrication.
    ToFabrication,
    /// In stock → consigned to a retailer.
    Consigned,
    /// Back into stock from fabrication or consignment.
    Returned,
    Sold,
}

impl MilestoneKind {
    pub const fn name(self) -> &'static str {
        match self {
            MilestoneKind::Bought => "bought",
            MilestoneKind::Shipped => "shipped",
            MilestoneKind::Arrived => "arrived",
            MilestoneKind::ToFabrication => "to_fabrication",
            MilestoneKind::Consigned => "consigned",
            MilestoneKind::Returned => "returned",
            MilestoneKind::Sold => "sold",
        }
    }

    const fn is_contract(self) -> bool {
        matches!(self, MilestoneKind::Bought | MilestoneKind::Sold)
    }

    /// Where a movement takes goods from, in the order it drains them, and
    /// where it puts them.
    const fn movement(self) -> Option<(&'static [Stage], Stage)> {
        match self {
            MilestoneKind::Shipped => Some((&[Stage::Bought], Stage::InTransit)),
            MilestoneKind::Arrived => Some((&[Stage::InTransit, Stage::Bought], Stage::InStock)),
            MilestoneKind::ToFabrication => Some((&[Stage::InStock], Stage::InFabrication)),
            MilestoneKind::Consigned => Some((&[Stage::InStock], Stage::Consigned)),
            MilestoneKind::Returned => {
                Some((&[Stage::InFabrication, Stage::Consigned], Stage::InStock))
            }
            MilestoneKind::Bought | MilestoneKind::Sold => None,
        }
    }
}

/// The order a sale takes goods: stock first, then consigned goods the
/// retailer sold, then cargo afloat, then what is bought but not delivered,
/// then work in fabrication.
const SALE_ORDER: [Stage; 5] = [
    Stage::InStock,
    Stage::Consigned,
    Stage::InTransit,
    Stage::Bought,
    Stage::InFabrication,
];

/// One event on one deal, as the desk receives it. Amounts are decimal
/// strings at the unit's scale so they cross JSON exactly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Milestone {
    pub deal: String,
    pub kind: MilestoneKind,
    pub goods: Goods,
    /// In the goods' unit: "2500.000" grams.
    pub quantity: String,
    /// Bought: the purchase cost. Sold: the proceeds. USDC. Absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usdc: Option<String>,
    /// The other side as named on the desk: supplier, carrier, vault, buyer.
    pub counterparty: String,
    /// The document: contract id, bill of lading, vault receipt.
    pub document: String,
    /// The address that signed this event, lowercase hex; empty when the
    /// owner entered it without a counterparty's signature.
    #[serde(default)]
    pub signer: String,
    /// Unix seconds.
    pub at: i64,
}

impl Milestone {
    /// The bytes a leaf commits to. One `key=value` per line, amounts as
    /// minor units with their unit, so "2500" and "2500.000" commit alike.
    /// [`Desk::apply`] refuses any field containing a control character, so
    /// the lines cannot be forged by a value that carries its own.
    pub fn canonical(&self) -> Result<Vec<u8>, DeskError> {
        let q = Money::parse(&self.quantity, self.goods.unit())?;
        let usdc = match &self.usdc {
            Some(s) => Money::parse(s, Currency::Usdc)?.to_minor().to_string(),
            None => "-".into(),
        };
        Ok(format!(
            "tameion-desk/v1\ndeal={}\nkind={}\ngoods={}\nquantity={} {}\nusdc={}\ncounterparty={}\ndocument={}\nsigner={}\nat={}\n",
            self.deal,
            self.kind.name(),
            self.goods.name(),
            q.to_minor(),
            q.currency(),
            usdc,
            self.counterparty,
            self.document,
            self.signer,
            self.at,
        )
        .into_bytes())
    }
}

/// Why a milestone was refused. Nothing is posted when one is returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeskError {
    Field {
        field: &'static str,
        why: &'static str,
    },
    UnknownDeal(String),
    DealExists(String),
    WrongGoods {
        deal: String,
        booked: Goods,
    },
    /// More than the stages hold: you cannot ship what was not bought.
    Exceeds {
        stage: String,
        holds: Money,
        asked: Money,
    },
    Money(MoneyError),
    Post(PostError),
    Balance(BalanceError),
}

impl From<MoneyError> for DeskError {
    fn from(e: MoneyError) -> Self {
        DeskError::Money(e)
    }
}
impl From<PostError> for DeskError {
    fn from(e: PostError) -> Self {
        DeskError::Post(e)
    }
}
impl From<BalanceError> for DeskError {
    fn from(e: BalanceError) -> Self {
        DeskError::Balance(e)
    }
}

impl fmt::Display for DeskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeskError::Field { field, why } => write!(f, "{field}: {why}"),
            DeskError::UnknownDeal(d) => {
                write!(f, "no deal {d} on this desk; book its purchase first")
            }
            DeskError::DealExists(d) => write!(f, "deal {d} is already booked"),
            DeskError::WrongGoods { deal, booked } => {
                write!(f, "deal {deal} is booked in {}", booked.name())
            }
            DeskError::Exceeds {
                stage,
                holds,
                asked,
            } => write!(f, "{asked} asked, but {stage} holds {holds}"),
            DeskError::Money(e) => write!(f, "amount: {e}"),
            DeskError::Post(e) => write!(f, "entry: {e}"),
            DeskError::Balance(e) => write!(f, "balance: {e:?}"),
        }
    }
}

impl std::error::Error for DeskError {}

/// A deal as booked by its purchase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deal {
    pub id: String,
    pub goods: Goods,
    pub bought: Money,
    pub cost: Money,
    pub supplier: String,
}

/// Where a deal's goods are, in the goods' unit, by stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Holding {
    pub by_stage: BTreeMap<Stage, Money>,
}

impl Holding {
    pub fn at(&self, s: Stage) -> Money {
        self.by_stage
            .get(&s)
            .copied()
            .unwrap_or(Money::zero(Currency::GoldGram))
    }

    pub fn total(&self) -> Result<Money, MoneyError> {
        let unit = self
            .by_stage
            .values()
            .next()
            .map_or(Currency::GoldGram, |m| m.currency());
        Money::sum(self.by_stage.values().copied(), unit)
    }

    /// Grams in one lender class.
    pub fn class(&self, c: Class) -> Result<Money, MoneyError> {
        let unit = self
            .by_stage
            .values()
            .next()
            .map_or(Currency::GoldGram, |m| m.currency());
        Money::sum(
            self.by_stage
                .iter()
                .filter(|(s, _)| s.class() == c)
                .map(|(_, m)| *m),
            unit,
        )
    }

    /// The stage a person would name: the one holding the most.
    pub fn stage(&self) -> &'static str {
        self.by_stage
            .iter()
            .filter(|(_, m)| m.to_minor() > 0)
            .max_by_key(|(_, m)| m.to_minor())
            .map_or("sold", |(s, _)| s.words())
    }
}

/// The book.
#[derive(Debug, Clone)]
pub struct Desk {
    chart: Chart,
    /// One journal per unit, so each balances on its own (grams, USDC).
    journals: BTreeMap<Currency, Journal>,
    deals: BTreeMap<String, Deal>,
    events: Vec<Milestone>,
    /// Per good, stock the trader keeps unhedged because they replace it
    /// every day (`docs/84` §1: Tomei "buys back the equivalent quantity of
    /// gold daily"). The owner sets it; the offset covers only what is above.
    base: BTreeMap<Goods, Money>,
}

impl Default for Desk {
    fn default() -> Self {
        Desk::new()
    }
}

fn acct(kind: &str, deal: &str) -> AccountId {
    AccountId::new(format!("{kind}:{deal}"))
}

fn check_text(field: &'static str, s: &str, max: usize, empty_ok: bool) -> Result<(), DeskError> {
    if !empty_ok && s.trim().is_empty() {
        return Err(DeskError::Field {
            field,
            why: "is empty",
        });
    }
    if s.len() > max {
        return Err(DeskError::Field {
            field,
            why: "is too long",
        });
    }
    // Line and paragraph separators split lines in some readers (docs/89 L2); direction overrides and
    // invisible marks make a document number show other than what was signed (docs/100 X6).
    if s.chars().any(|c| {
        c.is_control()
            || matches!(c, '\u{2028}' | '\u{2029}' | '\u{061C}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
    }) {
        return Err(DeskError::Field {
            field,
            why: "has a control character",
        });
    }
    Ok(())
}

impl Desk {
    pub fn new() -> Self {
        Desk {
            chart: Chart::new(),
            journals: BTreeMap::new(),
            deals: BTreeMap::new(),
            events: Vec::new(),
            base: BTreeMap::new(),
        }
    }

    /// Rebuild a desk from its milestones, in order. The same list always
    /// gives the same book and the same root.
    pub fn replay<'a>(events: impl IntoIterator<Item = &'a Milestone>) -> Result<Desk, DeskError> {
        let mut d = Desk::new();
        for m in events {
            d.apply(m.clone())?;
        }
        Ok(d)
    }

    pub fn deals(&self) -> impl Iterator<Item = &Deal> {
        self.deals.values()
    }

    pub fn deal(&self, id: &str) -> Option<&Deal> {
        self.deals.get(id)
    }

    pub fn events(&self) -> &[Milestone] {
        &self.events
    }

    /// The journal in one unit; empty when nothing was booked in it.
    pub fn journal(&self, ccy: Currency) -> Journal {
        self.journals.get(&ccy).cloned().unwrap_or_default()
    }

    /// The owner's unhedged base for `goods` (zero unless set).
    pub fn base(&self, goods: Goods) -> Money {
        self.base
            .get(&goods)
            .copied()
            .unwrap_or(Money::zero(goods.unit()))
    }

    /// Set the unhedged base. A policy, not a milestone: it moves no goods.
    pub fn set_base(&mut self, goods: Goods, grams: &str) -> Result<(), DeskError> {
        let b = Money::parse(grams, goods.unit())?;
        if b.is_negative() {
            return Err(DeskError::Field {
                field: "base",
                why: "cannot be negative",
            });
        }
        self.base.insert(goods, b);
        Ok(())
    }

    fn open(&mut self, id: AccountId, kind: AccountKind, ccy: Currency) {
        if self.chart.get(&id).is_none() {
            self.chart = std::mem::take(&mut self.chart).with(Account::new(id, kind, ccy));
        }
    }

    fn bal(&self, id: &AccountId) -> Result<Money, DeskError> {
        let ccy = self
            .chart
            .get(id)
            .ok_or_else(|| BalanceError::UnknownAccount(id.clone()))?
            .currency();
        Ok(self.journal(ccy).balance(&self.chart, id)?)
    }

    /// Validate, post and record one milestone. All or nothing: on an error
    /// the book is unchanged.
    pub fn apply(&mut self, m: Milestone) -> Result<Vec<Entry>, DeskError> {
        check_text("deal", &m.deal, 64, false)?;
        if !m
            .deal
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        {
            return Err(DeskError::Field {
                field: "deal",
                why: "use letters, digits, '-', '_' or '.'",
            });
        }
        check_text("counterparty", &m.counterparty, 80, false)?;
        check_text("document", &m.document, 120, false)?;
        // A document number is printable ASCII: a look-alike (Cyrillic, full-width, an invisible mark) would book the same
        // step twice under a number that reads the same (docs/101 Y6).
        if !m.document.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
            return Err(DeskError::Field {
                field: "document",
                why: "use letters, digits and ordinary punctuation (printable ASCII)",
            });
        }
        check_text("signer", &m.signer, 42, true)?;
        // Unix seconds from 2001 to 3000: a signed 0 or a negative time is a
        // mistake, and the signed value is unsigned (docs/89 L1).
        if !(1_000_000_000..=32_503_680_000).contains(&m.at) {
            return Err(DeskError::Field {
                field: "at",
                why: "is not a time between 2001 and 3000",
            });
        }
        let unit = m.goods.unit();
        let q = Money::parse(&m.quantity, unit)?;
        if q.to_minor() <= 0 {
            return Err(DeskError::Field {
                field: "quantity",
                why: "must be more than zero",
            });
        }
        // A bound far above any one shipment keeps every later sum and
        // decimal product inside range (docs/89 M2): 100 tonnes per step.
        if q.to_minor() > MAX_STEP_MINOR {
            return Err(DeskError::Field {
                field: "quantity",
                why: "is more than 100 tonnes in one step",
            });
        }
        let usdc = match (&m.usdc, m.kind.is_contract()) {
            (Some(s), true) => {
                let v = Money::parse(s, Currency::Usdc)?;
                if v.to_minor() > MAX_USDC_MINOR {
                    return Err(DeskError::Field {
                        field: "usdc",
                        why: "is more than ten billion USDC",
                    });
                }
                if v.to_minor() <= 0 {
                    return Err(DeskError::Field {
                        field: "usdc",
                        why: "must be more than zero",
                    });
                }
                Some(v)
            }
            (None, true) => {
                return Err(DeskError::Field {
                    field: "usdc",
                    why: "a contract carries its price",
                });
            }
            (Some(_), false) => {
                return Err(DeskError::Field {
                    field: "usdc",
                    why: "only a contract carries a price",
                });
            }
            (None, false) => None,
        };
        m.canonical()?; // every field encodes before anything is posted

        let deal_id = m.deal.clone();
        if m.kind == MilestoneKind::Bought {
            if self.deals.contains_key(&deal_id) {
                return Err(DeskError::DealExists(deal_id));
            }
        } else {
            // One step per deal, kind and document: a second tap signs a new time, not a new step (docs/100 X4).
            if self
                .events
                .iter()
                .any(|e| e.deal == m.deal && e.kind == m.kind && e.document == m.document)
            {
                return Err(DeskError::Field {
                    field: "document",
                    why: "this step of this deal is already booked under this document number; a second lot under one document takes its own number (e.g. CN-0610-2)",
                });
            }
            let d = self
                .deals
                .get(&deal_id)
                .ok_or_else(|| DeskError::UnknownDeal(deal_id.clone()))?;
            if d.goods != m.goods {
                return Err(DeskError::WrongGoods {
                    deal: deal_id,
                    booked: d.goods,
                });
            }
        }

        // Build every entry first; post only when all of them are valid.
        let mut work = self.clone();
        let doc = |kind| DocumentRef::new(kind, m.document.clone());
        let stage_acct = |s: Stage| acct(s.account(), &deal_id);
        let trading = AccountId::new(format!("trading:{}", m.goods.name()));
        let mut entries = Vec::new();

        match m.kind {
            MilestoneKind::Bought => {
                let cost = usdc.expect("checked above");
                for s in Stage::ALL {
                    work.open(stage_acct(s), AccountKind::Asset, unit);
                }
                for (id, kind, ccy) in [
                    (trading.clone(), AccountKind::Equity, unit),
                    (
                        acct("inventory", &deal_id),
                        AccountKind::Asset,
                        Currency::Usdc,
                    ),
                    (
                        acct("payable", &deal_id),
                        AccountKind::Liability,
                        Currency::Usdc,
                    ),
                    (
                        acct("receivable", &deal_id),
                        AccountKind::Asset,
                        Currency::Usdc,
                    ),
                    (
                        AccountId::new("revenue:sales"),
                        AccountKind::Revenue,
                        Currency::Usdc,
                    ),
                    (
                        AccountId::new("expense:cost-of-goods"),
                        AccountKind::Expense,
                        Currency::Usdc,
                    ),
                ] {
                    work.open(id, kind, ccy);
                }
                entries.push(Entry::post(
                    doc(DocumentKind::Contract),
                    vec![Debit::new(stage_acct(Stage::Bought), q)?],
                    vec![Credit::new(trading, q)?],
                )?);
                entries.push(Entry::post(
                    doc(DocumentKind::Contract),
                    vec![Debit::new(acct("inventory", &deal_id), cost)?],
                    vec![Credit::new(acct("payable", &deal_id), cost)?],
                )?);
                work.deals.insert(
                    deal_id.clone(),
                    Deal {
                        id: deal_id.clone(),
                        goods: m.goods,
                        bought: q,
                        cost,
                        supplier: m.counterparty.clone(),
                    },
                );
            }
            MilestoneKind::Sold => {
                let proceeds = usdc.expect("checked above");
                let held = work.holding(&deal_id)?;
                let total = held.total()?;
                let credits = work.drain(&held, &SALE_ORDER, q, "the deal", &deal_id)?;
                entries.push(Entry::post(
                    doc(DocumentKind::Contract),
                    vec![Debit::new(trading, q)?],
                    credits,
                )?);
                entries.push(Entry::post(
                    doc(DocumentKind::Contract),
                    vec![Debit::new(acct("receivable", &deal_id), proceeds)?],
                    vec![Credit::new(AccountId::new("revenue:sales"), proceeds)?],
                )?);
                // Cost leaves inventory in proportion to the goods; the last
                // sale takes whatever remains, so no residue is left behind.
                let inv = work.bal(&acct("inventory", &deal_id))?;
                let cost = if q == total {
                    inv
                } else {
                    Money::minor(
                        inv.to_minor() * q.to_minor() / total.to_minor(),
                        Currency::Usdc,
                    )
                };
                if cost.to_minor() > 0 {
                    entries.push(Entry::post(
                        doc(DocumentKind::Contract),
                        vec![Debit::new(AccountId::new("expense:cost-of-goods"), cost)?],
                        vec![Credit::new(acct("inventory", &deal_id), cost)?],
                    )?);
                }
            }
            movement => {
                let (from, to) = movement
                    .movement()
                    .expect("every non-contract kind is a movement");
                let held = work.holding(&deal_id)?;
                let words = from
                    .iter()
                    .map(|s| s.words())
                    .collect::<Vec<_>>()
                    .join(" + ");
                let credits = work.drain(&held, from, q, &words, &deal_id)?;
                entries.push(Entry::post(
                    doc(DocumentKind::Shipping),
                    vec![Debit::new(stage_acct(to), q)?],
                    credits,
                )?);
            }
        }

        for e in &entries {
            let ccy = e.debits()[0].amount().currency();
            work.journals.entry(ccy).or_default().append(e.clone());
        }
        work.events.push(m);
        *self = work;
        Ok(entries)
    }

    /// Credits taking `q` from `stages` in order, or `Exceeds`.
    fn drain(
        &self,
        held: &Holding,
        stages: &[Stage],
        q: Money,
        words: &str,
        deal: &str,
    ) -> Result<Vec<Credit>, DeskError> {
        let have = Money::sum(stages.iter().map(|s| held.at(*s)), q.currency())?;
        if q.to_minor() > have.to_minor() {
            return Err(DeskError::Exceeds {
                stage: words.into(),
                holds: have,
                asked: q,
            });
        }
        let mut left = q.to_minor();
        let mut credits = Vec::new();
        for s in stages {
            let take = left.min(held.at(*s).to_minor());
            if take > 0 {
                credits.push(Credit::new(
                    acct(s.account(), deal),
                    Money::minor(take, q.currency()),
                )?);
                left -= take;
            }
        }
        Ok(credits)
    }

    /// Where one deal's goods are.
    pub fn holding(&self, deal: &str) -> Result<Holding, DeskError> {
        if !self.deals.contains_key(deal) {
            return Err(DeskError::UnknownDeal(deal.into()));
        }
        let mut by_stage = BTreeMap::new();
        for s in Stage::ALL {
            by_stage.insert(s, self.bal(&acct(s.account(), deal))?);
        }
        Ok(Holding { by_stage })
    }

    /// Everything priced and still held, in the goods' unit: the long the
    /// offset has to answer for, before the unhedged base.
    pub fn exposure(&self, goods: Goods) -> Result<Money, DeskError> {
        let mut total = Money::zero(goods.unit());
        for d in self.deals.values().filter(|d| d.goods == goods) {
            total = total.checked_add(self.holding(&d.id)?.total()?)?;
        }
        Ok(total)
    }

    /// The short the venue should hold, in venue units (troy ounces for
    /// gold), before rounding to the market's size step: the exposure above
    /// the owner's unhedged base, never below zero.
    pub fn offset_target(&self, goods: Goods) -> Result<Decimal, DeskError> {
        let above = self.exposure(goods)?.checked_sub(self.base(goods))?;
        Ok(grams_to_contracts(goods, above).max(Decimal::ZERO))
    }

    /// The root over every milestone, in the order applied.
    pub fn root(&self) -> Result<Hash32, DeskError> {
        Ok(root_from_leaves(&self.leaves()?))
    }

    fn leaves(&self) -> Result<Vec<Hash32>, DeskError> {
        self.events
            .iter()
            .map(|m| Ok(leaf_from_canonical(&m.canonical()?)))
            .collect()
    }

    /// The RFC 9162 consistency path from the root over the first `m`
    /// milestones to the root now: the book only grew since then. `None`
    /// when `m` is 0 or more than the book holds.
    pub fn consistency(&self, m: usize) -> Result<Option<Vec<String>>, DeskError> {
        Ok(consistency_from_leaves(&self.leaves()?, m)
            .map(|p| p.iter().map(|h| h.to_hex()).collect()))
    }

    /// One deal's milestones with their paths to the root of the whole desk.
    pub fn disclose(&self, deal: &str) -> Result<Disclosure, DeskError> {
        if !self.deals.contains_key(deal) {
            return Err(DeskError::UnknownDeal(deal.into()));
        }
        let leaves = self.leaves()?;
        let root = root_from_leaves(&leaves);
        let mut items = Vec::new();
        for (i, m) in self
            .events
            .iter()
            .enumerate()
            .filter(|(_, m)| m.deal == deal)
        {
            let path = path_from_leaves(&leaves, i).unwrap_or_default();
            items.push(DisclosedMilestone {
                index: i,
                canonical: String::from_utf8(m.canonical()?).expect("canonical is utf-8"),
                path: path.iter().map(Step::from).collect(),
            });
        }
        Ok(Disclosure {
            root: root.to_hex(),
            leaves: leaves.len(),
            deal: deal.into(),
            items,
        })
    }

    /// What a lender reads about one deal (`docs/83` W7, `docs/84` §6): a
    /// borrowing-base certificate for the deal.
    ///
    /// `mark` is the venue's mark per contract (USDC per troy ounce);
    /// `offset_contracts` is the short the venue reports for this good across
    /// the whole desk, read back after the last order, which is split over
    /// deals in proportion to what each holds. `stress_bps` is the price fall
    /// the maximum loss is printed at; `basis_bps` the allowance for the
    /// goods not tracking the venue's market (grade, location, the lender's
    /// own reference price).
    pub fn statement(&self, deal: &str, p: &StatementInputs) -> Result<Statement, DeskError> {
        let d = self
            .deals
            .get(deal)
            .ok_or_else(|| DeskError::UnknownDeal(deal.into()))?
            .clone();
        let held = self.holding(deal)?;
        let held_total = held.total()?;
        let exposure = self.exposure(d.goods)?;
        let contracts = grams_to_contracts(d.goods, held_total);
        let value = contracts * p.mark;
        let share = if exposure.to_minor() > 0 {
            Decimal::from_i128_with_scale(held_total.to_minor(), 0)
                / Decimal::from_i128_with_scale(exposure.to_minor(), 0)
        } else {
            Decimal::ZERO
        };
        // Only a short offsets a long book; a long position adds to the
        // exposure and is never counted as protection (docs/89 M1).
        let short = (-p.offset_contracts).max(Decimal::ZERO);
        let offset = (short * share).min(contracts);
        let offset_share = if contracts.is_zero() {
            Decimal::ZERO
        } else {
            offset / contracts
        };
        let bps = |b: u32| Decimal::from(b) / dec!(10000);
        let unoffset_value = (contracts - offset) * p.mark;
        let max_loss = unoffset_value * bps(p.stress_bps) + value * bps(p.basis_bps);
        let usd = |x: Decimal| {
            x.round_dp_with_strategy(6, RoundingStrategy::MidpointAwayFromZero)
                .to_string()
        };
        let class_line = |c: Class| -> Result<ClassLine, DeskError> {
            let g = held.class(c)?;
            Ok(ClassLine {
                grams: g.to_string(),
                value_usdc: usd(grams_to_contracts(d.goods, g) * p.mark),
            })
        };
        let eligible = class_line(Class::Eligible)?;
        let eligible_value: Decimal = eligible.value_usdc.parse().unwrap_or(Decimal::ZERO);
        Ok(Statement {
            deal: d.id.clone(),
            goods: d.goods,
            market: d.goods.market().into(),
            stage: held.stage().into(),
            held: held_total.to_string(),
            held_contracts: format!("{:.6}", contracts.round_dp(6)),
            by_stage: held
                .by_stage
                .iter()
                .filter(|(_, m)| m.to_minor() > 0)
                .map(|(s, m)| StageLine {
                    stage: *s,
                    words: s.words().into(),
                    class: s.class(),
                    grams: m.to_string(),
                })
                .collect(),
            eligible,
            consigned: class_line(Class::Consigned)?,
            ineligible: class_line(Class::Ineligible)?,
            unhedged_base: self.base(d.goods).to_string(),
            cost_usdc: d.cost.to_string(),
            mark_usdc: p.mark.to_string(),
            value_usdc: usd(value),
            offset_contracts: format!("{:.6}", offset.round_dp(6)),
            short_contracts: format!("{:.6}", short.round_dp(6)),
            desk_held_contracts: format!(
                "{:.6}",
                grams_to_contracts(d.goods, exposure).round_dp(6)
            ),
            offset_share_bps: (offset_share * dec!(10000)).round().to_string(),
            stress_bps: p.stress_bps,
            basis_bps: p.basis_bps,
            max_loss_usdc: usd(max_loss),
            eligible_usdc: usd((eligible_value - max_loss).max(Decimal::ZERO)),
            venue_account: p.venue_account.clone(),
            hedge_control: p.hedge_control.clone(),
            funding_usdc: usd(p.funding_usdc * share),
            at: p.at,
            root: self.root()?.to_hex(),
            milestones: self
                .events
                .iter()
                .filter(|m| m.deal == deal)
                .map(|m| StatementMilestone {
                    kind: m.kind,
                    quantity: m.quantity.clone(),
                    document: m.document.clone(),
                    counterparty: m.counterparty.clone(),
                    signer: m.signer.clone(),
                    at: m.at,
                })
                .collect(),
            assumptions: format!(
                "Value at the venue's mark for {market}. Eligible: stock and goods in transit; consigned goods are their own class; goods not yet delivered and work in fabrication are not eligible. Maximum loss: a {stress}% fall on the part not offset, plus {basis}% of the whole for the goods not tracking {market} (grade, location, the lender's reference price). Eligible value after the maximum loss = the eligible class less the whole maximum loss. The offset is the short read back from the venue account, split across deals by what each holds; anyone can read that account's fills from the venue's public API. Funding on the offset since the deal was bought is the venue's own record for the account, this deal's share by grams; positive means the market paid for holding the offset. The lender's own advance rates are applied by the lender.",
                market = d.goods.market(),
                stress = Decimal::from(p.stress_bps) / dec!(100),
                basis = Decimal::from(p.basis_bps) / dec!(100),
            ),
        })
    }
}

/// Grams to venue contracts, exact to the decimal.
pub fn grams_to_contracts(goods: Goods, grams: Money) -> Decimal {
    let g = Decimal::from_i128_with_scale(grams.to_minor(), grams.currency().scale() as u32);
    g / goods.units_per_contract()
}

/// The numbers a statement is computed from that the book does not hold.
#[derive(Debug, Clone, PartialEq)]
pub struct StatementInputs {
    pub mark: Decimal,
    pub offset_contracts: Decimal,
    pub stress_bps: u32,
    pub basis_bps: u32,
    pub venue_account: String,
    /// Who can move the offset, in words (`docs/84` §6: a lender wants the
    /// hedge in an account it controls or is pledged).
    pub hedge_control: String,
    /// Funding the venue paid (+) or charged (−) on the account's short for
    /// this good since the deal was bought, in USDC: the measured cost of the
    /// protection (`docs/73` T3). Account-wide; the statement takes the
    /// deal's share by what it holds.
    pub funding_usdc: Decimal,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatementMilestone {
    pub kind: MilestoneKind,
    pub quantity: String,
    pub document: String,
    pub counterparty: String,
    pub signer: String,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StageLine {
    pub stage: Stage,
    pub words: String,
    pub class: Class,
    pub grams: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassLine {
    pub grams: String,
    pub value_usdc: String,
}

/// One deal, as a lender reads it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Statement {
    pub deal: String,
    pub goods: Goods,
    pub market: String,
    pub stage: String,
    pub held: String,
    pub held_contracts: String,
    pub by_stage: Vec<StageLine>,
    pub eligible: ClassLine,
    pub consigned: ClassLine,
    pub ineligible: ClassLine,
    pub unhedged_base: String,
    pub cost_usdc: String,
    pub mark_usdc: String,
    pub value_usdc: String,
    pub offset_contracts: String,
    /// The short read back from the venue for this good, across the desk, and
    /// the desk's whole holding of it in venue units: a lender recomputes this
    /// deal's share of the short from them, and sees what else it backs
    /// (docs/92 T4).
    pub short_contracts: String,
    pub desk_held_contracts: String,
    pub offset_share_bps: String,
    pub stress_bps: u32,
    pub basis_bps: u32,
    pub max_loss_usdc: String,
    pub eligible_usdc: String,
    pub venue_account: String,
    pub hedge_control: String,
    /// This deal's share of the funding on the short since it was bought;
    /// positive = the market paid the trader for holding the offset.
    pub funding_usdc: String,
    pub at: i64,
    pub root: String,
    pub milestones: Vec<StatementMilestone>,
    pub assumptions: String,
}

/// A path step as hex, for JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub sibling: String,
    pub left: bool,
}

impl From<&ProofStep> for Step {
    fn from(s: &ProofStep) -> Self {
        Step {
            sibling: s.sibling.to_hex(),
            left: s.sibling_is_left,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisclosedMilestone {
    pub index: usize,
    /// The exact bytes the leaf commits to.
    pub canonical: String,
    pub path: Vec<Step>,
}

/// One deal's milestones, provable against the desk's root without the rest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Disclosure {
    pub root: String,
    pub leaves: usize,
    pub deal: String,
    pub items: Vec<DisclosedMilestone>,
}

impl Disclosure {
    /// Recompute every disclosed leaf from its bytes and walk its path to
    /// the root. Says which item failed, and why.
    pub fn verify(&self) -> Result<(), String> {
        let root = Hash32::from_hex(&self.root).ok_or("root is not 32 bytes of hex")?;
        if self.items.is_empty() {
            return Err("nothing disclosed".into());
        }
        for it in &self.items {
            if !it.canonical.contains(&format!("\ndeal={}\n", self.deal)) {
                return Err(format!("item {} is not about deal {}", it.index, self.deal));
            }
            let path: Vec<ProofStep> = it
                .path
                .iter()
                .map(|s| {
                    Hash32::from_hex(&s.sibling)
                        .map(|sibling| ProofStep {
                            sibling,
                            sibling_is_left: s.left,
                        })
                        .ok_or_else(|| {
                            format!("item {}: a path step is not 32 bytes of hex", it.index)
                        })
                })
                .collect::<Result<_, _>>()?;
            if !ledger::verify_inclusion(leaf_from_canonical(it.canonical.as_bytes()), &path, root)
            {
                return Err(format!("item {} does not reach the root", it.index));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(deal: &str, kind: MilestoneKind, q: &str, usdc: Option<&str>, doc: &str) -> Milestone {
        Milestone {
            deal: deal.into(),
            kind,
            goods: Goods::Gold,
            quantity: q.into(),
            usdc: usdc.map(Into::into),
            counterparty: "Supplier".into(),
            document: doc.into(),
            signer: String::new(),
            at: 1_792_000_000,
        }
    }

    fn g(s: &str) -> Money {
        Money::parse(s, Currency::GoldGram).unwrap()
    }

    fn inputs(offset: Decimal) -> StatementInputs {
        StatementInputs {
            mark: dec!(4000),
            offset_contracts: offset,
            stress_bps: 3000,
            basis_bps: 200,
            venue_account: "0xabc".into(),
            hedge_control: "trade-only key held by the trader".into(),
            funding_usdc: dec!(2),
            at: 1,
        }
    }

    #[test]
    fn a_shipment_moves_through_its_stages_and_the_books_balance() {
        let mut d = Desk::new();
        d.apply(m(
            "D1",
            MilestoneKind::Bought,
            "1000",
            Some("133000"),
            "PO-1",
        ))
        .unwrap();
        assert_eq!(d.exposure(Goods::Gold).unwrap(), g("1000"));
        d.apply(m("D1", MilestoneKind::Shipped, "1000", None, "BL-1"))
            .unwrap();
        d.apply(m("D1", MilestoneKind::Arrived, "600", None, "AN-1"))
            .unwrap();
        let h = d.holding("D1").unwrap();
        assert_eq!(
            (h.at(Stage::InTransit), h.at(Stage::InStock)),
            (g("400"), g("600"))
        );
        assert_eq!(h.stage(), "in stock");
        // Movements shift goods between stages; the exposure is unchanged.
        assert_eq!(d.exposure(Goods::Gold).unwrap(), g("1000"));
        d.apply(m("D1", MilestoneKind::Sold, "700", Some("95000"), "SO-1"))
            .unwrap();
        let h = d.holding("D1").unwrap();
        assert_eq!(
            (h.at(Stage::InTransit), h.at(Stage::InStock)),
            (g("300"), g("0")),
            "stock sold first, then cargo afloat"
        );
        for ccy in [Currency::GoldGram, Currency::Usdc] {
            let (dr, cr) = d.journal(ccy).trial_balance(ccy).unwrap();
            assert_eq!(dr, cr, "{ccy} books balance");
        }
        d.apply(m("D1", MilestoneKind::Sold, "300", Some("41000"), "SO-2"))
            .unwrap();
        assert_eq!(d.exposure(Goods::Gold).unwrap(), g("0"));
        assert!(
            d.bal(&acct("inventory", "D1")).unwrap().is_zero(),
            "no cost residue"
        );
    }

    #[test]
    fn gold_flies_into_a_vault_goes_to_fabrication_and_consignment() {
        // docs/84 §1b: gold travels by air courier into a vault, not by ship.
        let mut d = Desk::new();
        d.apply(m("G", MilestoneKind::Bought, "1000", Some("133000"), "PO"))
            .unwrap();
        d.apply(m("G", MilestoneKind::Arrived, "1000", None, "VAULT-RCPT"))
            .unwrap(); // straight from the supplier
        d.apply(m("G", MilestoneKind::ToFabrication, "300", None, "JOB-1"))
            .unwrap();
        d.apply(m("G", MilestoneKind::Consigned, "200", None, "CONS-1"))
            .unwrap();
        let h = d.holding("G").unwrap();
        assert_eq!(h.class(Class::Eligible).unwrap(), g("500"));
        assert_eq!(h.class(Class::Ineligible).unwrap(), g("300"));
        assert_eq!(h.class(Class::Consigned).unwrap(), g("200"));
        assert!(matches!(
            d.apply(m("G", MilestoneKind::Consigned, "600", None, "CONS-2")),
            Err(DeskError::Exceeds { .. })
        ));
        d.apply(m("G", MilestoneKind::Returned, "300", None, "JOB-1-DONE"))
            .unwrap();
        assert_eq!(
            d.holding("G").unwrap().class(Class::Ineligible).unwrap(),
            g("0")
        );
        // A sale takes stock first, then consigned goods.
        d.apply(m("G", MilestoneKind::Sold, "900", Some("130000"), "SO"))
            .unwrap();
        let h = d.holding("G").unwrap();
        assert_eq!(
            (h.at(Stage::InStock), h.at(Stage::Consigned)),
            (g("0"), g("100"))
        );
        let s = d.statement("G", &inputs(Decimal::ZERO)).unwrap();
        assert_eq!(s.consigned.grams, "100.000 g gold");
        assert_eq!(s.eligible.grams, "0.000 g gold");
        assert_eq!(
            s.eligible_usdc, "0",
            "nothing eligible once only consigned goods remain"
        );
    }

    #[test]
    fn a_step_twice_and_a_hidden_direction_mark_are_refused() {
        // docs/100 X4: the same step of the same deal under the same document books once.
        let mut d = Desk::new();
        d.apply(m("D1", MilestoneKind::Bought, "4", Some("536"), "PO"))
            .unwrap();
        d.apply(m("D1", MilestoneKind::Arrived, "4", None, "VR"))
            .unwrap();
        d.apply(m("D1", MilestoneKind::Consigned, "4", None, "CN"))
            .unwrap();
        d.apply(m("D1", MilestoneKind::Returned, "4", None, "RT"))
            .unwrap();
        let before = d.root().unwrap();
        assert!(
            d.apply(m("D1", MilestoneKind::Returned, "4", None, "RT"))
                .is_err(),
            "booked twice"
        );
        assert_eq!(d.root().unwrap(), before);
        // docs/100 X6: a right-to-left override shows a document number other than the one signed.
        assert!(
            d.apply(m(
                "D1",
                MilestoneKind::Consigned,
                "1",
                None,
                "CN-\u{202E}0160"
            ))
            .is_err()
        );
        // docs/101 Y6: a look-alike of RT (Cyrillic Т, full-width R) or an invisible mark is not a new document number.
        for doc in ["R\u{0422}", "RT\u{2060}", "RT\u{00AD}", "\u{FF32}T"] {
            assert!(
                d.apply(m("D1", MilestoneKind::Returned, "4", None, doc))
                    .is_err(),
                "{doc:?}"
            );
        }
    }

    #[test]
    fn impossible_milestones_are_refused_and_change_nothing() {
        let mut d = Desk::new();
        assert!(matches!(
            d.apply(m("D1", MilestoneKind::Shipped, "1", None, "BL")),
            Err(DeskError::UnknownDeal(_))
        ));
        d.apply(m("D1", MilestoneKind::Bought, "100", Some("13300"), "PO"))
            .unwrap();
        let before = d.root().unwrap();
        assert!(matches!(
            d.apply(m("D1", MilestoneKind::ToFabrication, "1", None, "J")),
            Err(DeskError::Exceeds { .. })
        ));
        assert!(matches!(
            d.apply(m("D1", MilestoneKind::Shipped, "100.001", None, "BL")),
            Err(DeskError::Exceeds { .. })
        ));
        assert!(matches!(
            d.apply(m("D1", MilestoneKind::Bought, "1", Some("1"), "PO")),
            Err(DeskError::DealExists(_))
        ));
        assert!(
            d.apply(m("D1", MilestoneKind::Sold, "1", None, "SO"))
                .is_err(),
            "a sale carries its price"
        );
        assert!(
            d.apply(m("D1", MilestoneKind::Shipped, "1", Some("5"), "BL"))
                .is_err(),
            "movements carry no price"
        );
        assert!(
            d.apply(m("D1", MilestoneKind::Shipped, "1", None, "BL\ndeal=D2"))
                .is_err(),
            "no forged lines"
        );
        assert!(
            d.apply(m("D1", MilestoneKind::Shipped, "0.0001", None, "BL"))
                .is_err(),
            "finer than a milligram"
        );
        assert!(
            d.apply(m(
                "D1",
                MilestoneKind::Shipped,
                "1",
                None,
                "BL\u{2028}deal=D2"
            ))
            .is_err(),
            "no line separators"
        );
        let mut early = m("D1", MilestoneKind::Shipped, "1", None, "BL");
        early.at = 0;
        assert!(d.apply(early).is_err(), "a time of 0 is refused");
        assert_eq!(d.root().unwrap(), before);
        assert_eq!(d.events().len(), 1);
    }

    #[test]
    fn the_offset_covers_only_what_is_above_the_unhedged_base() {
        let mut d = Desk::new();
        d.apply(m(
            "D1",
            MilestoneKind::Bought,
            "311.034",
            Some("41000"),
            "PO",
        ))
        .unwrap();
        // 311.034 g is 9.99998... oz; exact decimal division, no float.
        assert_eq!(
            d.offset_target(Goods::Gold).unwrap().round_dp(5),
            dec!(9.99998)
        );
        assert_eq!(
            grams_to_contracts(Goods::Gold, g("31.103")).round_dp(3),
            dec!(1.000)
        );
        d.set_base(Goods::Gold, "100").unwrap();
        assert_eq!(
            d.offset_target(Goods::Gold).unwrap().round_dp(4),
            dec!(6.7849)
        );
        d.set_base(Goods::Gold, "500").unwrap();
        assert_eq!(
            d.offset_target(Goods::Gold).unwrap(),
            Decimal::ZERO,
            "never a long"
        );
        assert!(d.set_base(Goods::Gold, "-1").is_err());
    }

    #[test]
    fn the_statement_splits_the_offset_and_prints_the_maximum_loss() {
        let mut d = Desk::new();
        d.apply(m(
            "A",
            MilestoneKind::Bought,
            "311.035",
            Some("41000"),
            "PO-A",
        ))
        .unwrap();
        d.apply(m("A", MilestoneKind::Arrived, "311.035", None, "VR-A"))
            .unwrap();
        d.apply(m(
            "B",
            MilestoneKind::Bought,
            "311.035",
            Some("41000"),
            "PO-B",
        ))
        .unwrap();
        let s = d.statement("A", &inputs(dec!(-10))).unwrap();
        // Each deal holds ~10 oz worth ~40,000; the venue's 10-oz short is split 5/5.
        assert_eq!(s.offset_contracts, "5.000000");
        assert_eq!(s.offset_share_bps, "5000");
        // unoffset 5 oz × 4000 × 30% = 6,000 + basis 2% × 40,000 = 800.
        let ml: Decimal = s.max_loss_usdc.parse().unwrap();
        assert!((ml - dec!(6800)).abs() < dec!(1), "{ml}");
        let el: Decimal = s.eligible_usdc.parse().unwrap();
        assert!(
            (el - dec!(33200)).abs() < dec!(1),
            "in stock, eligible: 40,000 - 6,800 = {el}"
        );
        assert_eq!(s.root, d.root().unwrap().to_hex());
        // Deal B is bought, not delivered: worth the same, eligible for nothing.
        let sb = d.statement("B", &inputs(dec!(-10))).unwrap();
        assert_eq!(sb.eligible_usdc, "0");
        assert_eq!(sb.ineligible.grams, "311.035 g gold");
    }

    #[test]
    fn a_long_position_is_never_counted_as_offset_and_huge_steps_are_refused() {
        let mut d = Desk::new();
        d.apply(m(
            "A",
            MilestoneKind::Bought,
            "311.035",
            Some("41000"),
            "PO-A",
        ))
        .unwrap();
        let s = d.statement("A", &inputs(dec!(10))).unwrap();
        assert_eq!(
            s.offset_share_bps, "0",
            "a long adds exposure, it does not protect"
        );
        assert!(
            d.apply(m(
                "B",
                MilestoneKind::Bought,
                "100000001",
                Some("1"),
                "PO-B"
            ))
            .is_err()
        );
        assert!(
            d.apply(m(
                "C",
                MilestoneKind::Bought,
                "1",
                Some("10000000001"),
                "PO-C"
            ))
            .is_err()
        );
        assert_eq!(d.events().len(), 1);
    }

    #[test]
    fn a_lender_checks_one_deal_without_the_others() {
        let mut d = Desk::new();
        d.apply(m("A", MilestoneKind::Bought, "100", Some("13300"), "PO-A"))
            .unwrap();
        d.apply(m("B", MilestoneKind::Bought, "50", Some("6650"), "PO-B"))
            .unwrap();
        d.apply(m("A", MilestoneKind::Shipped, "100", None, "BL-A"))
            .unwrap();
        d.apply(m("B", MilestoneKind::Shipped, "50", None, "BL-B"))
            .unwrap();
        d.apply(m("A", MilestoneKind::Arrived, "100", None, "AN-A"))
            .unwrap();
        let disc = d.disclose("A").unwrap();
        assert_eq!((disc.items.len(), disc.leaves), (3, 5));
        assert!(disc.items.iter().all(|i| !i.canonical.contains("deal=B")));
        disc.verify().unwrap();
        let mut bad = disc.clone();
        bad.items[1].canonical = bad.items[1]
            .canonical
            .replace("quantity=100000", "quantity=100001");
        assert!(bad.verify().is_err());
        assert_eq!(
            Desk::replay(d.events()).unwrap().root().unwrap().to_hex(),
            disc.root
        );
    }
}
