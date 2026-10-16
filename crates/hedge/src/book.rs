//! Booking hedge activity into the ledger as balanced double-entry lines.
//!
//! Everything is denominated in USDC because that is what the venue reports:
//! position value, unrealised PnL, margin used and funding are all USDC
//! figures in `clearinghouseState`. The base-unit size of a position lives in
//! the hedge's own types; the ledger sees value, as it should.
//!
//! Account names are fixed here so a typo cannot create a new account by
//! accident: `Journal::balance` refuses accounts that are not in the chart.

use ledger::{
    Account, AccountId, AccountKind, Chart, Credit, Currency, Debit, DocumentKind, DocumentRef,
    Entry, Journal, Money, MoneyError, PostError,
};
use rust_decimal::{Decimal, RoundingStrategy};

/// The accounts the hedge touches. All USDC.
pub mod accounts {
    use ledger::AccountId;

    pub fn treasury_usdc() -> AccountId {
        AccountId::new("asset:treasury:usdc")
    }
    /// USDC value of a spot holding, marked each cycle.
    pub fn spot(coin: &str) -> AccountId {
        AccountId::new(format!("asset:treasury:spot:{coin}"))
    }
    /// USDC posted to the venue as margin.
    pub fn hedge_margin() -> AccountId {
        AccountId::new("asset:hedge:margin")
    }
    /// Carrying value of a hedge's unrealised PnL. Goes negative through
    /// loss entries; it is an asset account, so a credit balance reads as a
    /// negative number, which is what an underwater hedge is.
    pub fn hedge_mtm(coin: &str) -> AccountId {
        AccountId::new(format!("asset:hedge:mtm:{coin}"))
    }
    pub fn spot_gain() -> AccountId {
        AccountId::new("revenue:spot:gain")
    }
    pub fn spot_loss() -> AccountId {
        AccountId::new("expense:spot:loss")
    }
    pub fn hedge_gain() -> AccountId {
        AccountId::new("revenue:hedge:gain")
    }
    pub fn hedge_loss() -> AccountId {
        AccountId::new("expense:hedge:loss")
    }
    pub fn funding_income() -> AccountId {
        AccountId::new("revenue:funding")
    }
    pub fn funding_expense() -> AccountId {
        AccountId::new("expense:funding")
    }
    /// Margin that reached the venue from outside these books (the funding
    /// script when it did not book, a transfer by hand, a testnet faucet),
    /// or left it the same way. Equity, so the books can say "this much is
    /// on the venue and we did not watch it arrive" without pretending the
    /// treasury account paid it.
    pub fn margin_contributed() -> AccountId {
        AccountId::new("equity:hedge:contributed")
    }
    pub fn fees() -> AccountId {
        AccountId::new("expense:fees")
    }
}

/// The chart of accounts for one coin's hedge plus the shared accounts.
/// Call once per coin and merge with `Chart::with` if hedging several.
pub fn chart(coins: &[&str]) -> Chart {
    let u = Currency::Usdc;
    let mut chart = Chart::new()
        .with(Account::new(
            accounts::treasury_usdc(),
            AccountKind::Asset,
            u,
        ))
        .with(Account::new(
            accounts::hedge_margin(),
            AccountKind::Asset,
            u,
        ))
        .with(Account::new(accounts::spot_gain(), AccountKind::Revenue, u))
        .with(Account::new(accounts::spot_loss(), AccountKind::Expense, u))
        .with(Account::new(
            accounts::hedge_gain(),
            AccountKind::Revenue,
            u,
        ))
        .with(Account::new(
            accounts::hedge_loss(),
            AccountKind::Expense,
            u,
        ))
        .with(Account::new(
            accounts::funding_income(),
            AccountKind::Revenue,
            u,
        ))
        .with(Account::new(
            accounts::funding_expense(),
            AccountKind::Expense,
            u,
        ))
        .with(Account::new(accounts::fees(), AccountKind::Expense, u))
        .with(Account::new(
            accounts::margin_contributed(),
            AccountKind::Equity,
            u,
        ));
    for coin in coins {
        chart = chart
            .with(Account::new(accounts::spot(coin), AccountKind::Asset, u))
            .with(Account::new(
                accounts::hedge_mtm(coin),
                AccountKind::Asset,
                u,
            ));
    }
    chart
}

/// Why a booking could not be made.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BookError {
    #[error("money: {0}")]
    Money(#[from] MoneyError),
    #[error("post: {0}")]
    Post(#[from] PostError),
}

/// Turn a venue decimal into exact USDC `Money`.
///
/// The venue reports USDC to six places, which is USDC's own scale, so this
/// is normally lossless. If a figure ever carries more places it is rounded
/// half-to-even here, once, at the boundary, and nowhere else.
// ponytail: single rounding point; if residue ever shows up in the trial
// balance, book the rounding to its own account instead of hiding it here.
pub fn usdc(amount: Decimal) -> Result<Money, MoneyError> {
    let rounded = amount.round_dp_with_strategy(6, RoundingStrategy::MidpointNearestEven);
    Money::parse(&rounded.normalize().to_string(), Currency::Usdc)
}

/// Move USDC from the treasury into the venue margin account.
pub fn deposit_margin(amount: Decimal, doc_id: &str) -> Result<Entry, BookError> {
    let m = usdc(amount)?;
    Ok(Entry::post(
        DocumentRef::new(DocumentKind::Hedge, doc_id),
        vec![Debit::new(accounts::hedge_margin(), m)?],
        vec![Credit::new(accounts::treasury_usdc(), m)?],
    )?)
}

/// Mark a spot holding. `delta_value` is new value minus last booked value.
/// A gain debits the holding and credits revenue; a loss the reverse. Zero
/// books nothing and returns `None`.
pub fn mark_spot(
    coin: &str,
    delta_value: Decimal,
    doc_id: &str,
) -> Result<Option<Entry>, BookError> {
    signed_pair(
        DocumentKind::Mark,
        doc_id,
        delta_value,
        accounts::spot(coin),
        accounts::spot_gain(),
        accounts::spot_loss(),
    )
}

/// Mark a hedge. `delta_upnl` is new unrealised PnL minus last booked.
pub fn mark_hedge(
    coin: &str,
    delta_upnl: Decimal,
    doc_id: &str,
) -> Result<Option<Entry>, BookError> {
    signed_pair(
        DocumentKind::Mark,
        doc_id,
        delta_upnl,
        accounts::hedge_mtm(coin),
        accounts::hedge_gain(),
        accounts::hedge_loss(),
    )
}

/// Book a funding payment. Positive is received (shorts are paid when
/// funding is positive), negative is paid.
pub fn funding(_coin: &str, usdc_delta: Decimal, doc_id: &str) -> Result<Option<Entry>, BookError> {
    signed_pair(
        DocumentKind::Funding,
        doc_id,
        usdc_delta,
        accounts::hedge_margin(),
        accounts::funding_income(),
        accounts::funding_expense(),
    )
}

/// Realised profit or loss and fees, net: how the venue's cash moved this
/// step beyond what marks and funding explain, on a step that traded.
/// Positive is cash in. Zero books nothing.
pub fn settle(usdc_delta: Decimal, doc_id: &str) -> Result<Option<Entry>, BookError> {
    signed_pair(
        DocumentKind::Hedge,
        doc_id,
        usdc_delta,
        accounts::hedge_margin(),
        accounts::hedge_gain(),
        accounts::hedge_loss(),
    )
}

/// Margin that moved between the venue and the outside world on a step that
/// did not trade: a deposit these books never saw, a withdrawal, a faucet.
/// Positive arrived on the venue. Booked against equity, not the treasury,
/// because the books did not watch the treasury pay it.
pub fn external(usdc_delta: Decimal, doc_id: &str) -> Result<Option<Entry>, BookError> {
    if usdc_delta.is_zero() {
        return Ok(None);
    }
    let m = usdc(usdc_delta.abs())?;
    if m.is_zero() {
        return Ok(None);
    }
    let doc = DocumentRef::new(DocumentKind::Hedge, doc_id);
    let entry = if usdc_delta.is_sign_positive() {
        Entry::post(
            doc,
            vec![Debit::new(accounts::hedge_margin(), m)?],
            vec![Credit::new(accounts::margin_contributed(), m)?],
        )?
    } else {
        Entry::post(
            doc,
            vec![Debit::new(accounts::margin_contributed(), m)?],
            vec![Credit::new(accounts::hedge_margin(), m)?],
        )?
    };
    Ok(Some(entry))
}

/// Book a trading fee paid out of margin.
pub fn fee(amount: Decimal, doc_id: &str) -> Result<Option<Entry>, BookError> {
    if amount.is_zero() {
        return Ok(None);
    }
    let m = usdc(amount.abs())?;
    Ok(Some(Entry::post(
        DocumentRef::new(DocumentKind::Hedge, doc_id),
        vec![Debit::new(accounts::fees(), m)?],
        vec![Credit::new(accounts::hedge_margin(), m)?],
    )?))
}

/// What the books say the treasury's net position in a coin is: spot value
/// plus the hedge's carrying value. Near zero means delta-neutral *and
/// booked that way*.
pub fn net_view(
    journal: &Journal,
    chart: &Chart,
    coin: &str,
) -> Result<NetView, ledger::BalanceError> {
    let spot = journal.balance(chart, &accounts::spot(coin))?;
    let hedge = journal.balance(chart, &accounts::hedge_mtm(coin))?;
    let margin = journal.balance(chart, &accounts::hedge_margin())?;
    let net = spot.checked_add(hedge)?;
    Ok(NetView {
        spot,
        hedge,
        margin,
        net,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetView {
    pub spot: Money,
    pub hedge: Money,
    pub margin: Money,
    pub net: Money,
}

fn signed_pair(
    kind: DocumentKind,
    doc_id: &str,
    delta: Decimal,
    asset: AccountId,
    gain: AccountId,
    loss: AccountId,
) -> Result<Option<Entry>, BookError> {
    if delta.is_zero() {
        return Ok(None);
    }
    let m = usdc(delta.abs())?;
    if m.is_zero() {
        // Rounded to nothing at six places. Not worth an entry.
        return Ok(None);
    }
    let doc = DocumentRef::new(kind, doc_id);
    let entry = if delta.is_sign_positive() {
        Entry::post(
            doc,
            vec![Debit::new(asset, m)?],
            vec![Credit::new(gain, m)?],
        )?
    } else {
        Entry::post(
            doc,
            vec![Debit::new(loss, m)?],
            vec![Credit::new(asset, m)?],
        )?
    };
    Ok(Some(entry))
}
