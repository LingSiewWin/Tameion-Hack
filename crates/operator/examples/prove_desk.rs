//! One gold deal through the desk, live (`docs/83` W6): a supplier, a carrier
//! and a buyer each sign their milestone; the book moves the grams between
//! stages; the offset on Hyperliquid testnet follows the book (opened at the
//! purchase, held while the gold is at sea, closed at the sale), read back
//! after every order; a lender's statement is signed while the gold is in
//! transit, and the desk's root is anchored on Arbitrum Sepolia so one deal
//! can be checked against the chain without the rest of the book.
//!
//! Env:
//! - `HEDGE_LIVE=1` and `~/.tameion/hedge-agent.key` (trade-only) to send
//!   orders; anything else is a dry run (`hedge::driver`).
//! - `HEDGE_USER` the Hyperliquid testnet account the key trades for.
//! - `DESK_GRAMS` (default 10.000) the deal size.
//! - `TAMEION_RPC_ARBITRUM_SEPOLIA`, `TREASURY_KEY_FILE` for the anchor.
//!
//! Run through `scripts/prove_desk.sh` so the output lands in `proof/`.

use alloy::primitives::{Address, B256};
use alloy::signers::local::PrivateKeySigner;
use desk::{Goods, Milestone, MilestoneKind};
use operator::desk::{self as d, Book};
use rust_decimal::{Decimal, RoundingStrategy};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

const CHAIN_ID: u64 = 421_614; // Arbitrum Sepolia: where the signatures and the anchor live
const ANCHOR_FILE: &str = "runtime/arbitrum-sepolia_anchor.addr";

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn home(p: &str) -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(p)
}

/// A counterparty's test key, made once under ~/.tameion/test-keys, mode 600.
fn party_key(name: &str) -> Result<PrivateKeySigner, String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let path = home(&format!(".tameion/test-keys/desk-{name}.key"));
    if let Ok(k) = std::fs::read_to_string(&path) {
        return PrivateKeySigner::from_str(k.trim()).map_err(|e| e.to_string());
    }
    let k = PrivateKeySigner::random();
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    writeln!(f, "0x{}", alloy::hex::encode(k.to_bytes())).map_err(|e| e.to_string())?;
    Ok(k)
}

fn milestone(
    deal: &str,
    kind: MilestoneKind,
    grams: &str,
    usdc: Option<String>,
    who: &str,
    doc: &str,
) -> Milestone {
    Milestone {
        deal: deal.into(),
        kind,
        goods: Goods::Gold,
        quantity: grams.into(),
        usdc,
        counterparty: who.into(),
        document: doc.into(),
        signer: String::new(),
        at: now(),
    }
}

async fn offset_and_read(book: &Book, user: Address, label: &str) -> Result<Decimal, String> {
    let target = book
        .desk
        .offset_target(Goods::Gold)
        .map_err(|e| e.to_string())?;
    println!("\n### offset after {label}");
    println!(
        "book exposure: {} → target short {} PAXG (troy oz)",
        book.desk.exposure(Goods::Gold).map_err(|e| e.to_string())?,
        target.round_dp(6)
    );
    let r = d::offset(&book.desk, Goods::Gold, user).await?;
    for c in &r.coins {
        println!(
            "{}: mark {} · before {} · target {} · rebalance {:?}",
            c.coin, c.mark_px, c.hedge_size_before, c.target_size, c.rebalance
        );
        if let Some(v) = &c.verdict {
            println!("  verdict: {v:?}");
        }
        println!("  action: {:?}", c.action);
    }
    println!(
        "account: value {} · margin used {} · maintenance {} · health {:?} · halted before: {}",
        r.account.account_value,
        r.account.margin_used,
        r.account.maintenance_margin_used,
        r.health,
        r.halted_before
    );
    let read = d::venue_position(user, "PAXG").await?;
    println!("read back from the venue: PAXG position {read}");
    Ok(read)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let user = Address::from_str(
        &std::env::var("HEDGE_USER").map_err(|_| "set HEDGE_USER to the testnet account")?,
    )?;
    let grams = std::env::var("DESK_GRAMS").unwrap_or_else(|_| "10.000".into());
    let stamp = now();
    let deal = format!("GOLD-{stamp}");
    let book_path = PathBuf::from(format!("runtime/desk/proof-{stamp}.jsonl"));
    println!(
        "# desk proof · deal {deal} · {grams} g gold · book {}",
        book_path.display()
    );
    println!(
        "# venue: Hyperliquid testnet, account {user:#x}, mode {}",
        if std::env::var("HEDGE_LIVE").as_deref() == Ok("1") {
            "LIVE"
        } else {
            "dry run"
        }
    );

    let supplier = party_key("supplier")?;
    let carrier = party_key("carrier")?;
    let buyer = party_key("buyer")?;
    let mallory = PrivateKeySigner::random();
    // The example's desk id: the venue account (a service desk uses its account address).
    let desk_id = user;
    let mut book = Book::open(&book_path, CHAIN_ID, desk_id)?;
    for (name, k) in [
        ("Supplier", &supplier),
        ("Carrier", &carrier),
        ("Buyer", &buyer),
    ] {
        book.approve(name, &format!("{:#x}", k.address()), None)?;
        println!("approved by the owner: {name} signs as {:#x}", k.address());
    }

    // Price the purchase at the venue's mark, as a supplier quoting spot would.
    let mark = d::venue_mark("PAXG").await?;
    let oz = desk::grams_to_contracts(
        Goods::Gold,
        ledger::Money::parse(&grams, Goods::Gold.unit())?,
    );
    let usd = |x: Decimal| {
        x.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero)
            .to_string()
    };
    let cost = usd(oz * mark);
    println!(
        "\nPAXG mark {mark} USDC/oz · {grams} g = {} oz · cost {cost} USDC",
        oz.round_dp(6)
    );

    println!("\n## 1. bought (signed by the supplier)");
    let m = milestone(
        &deal,
        MilestoneKind::Bought,
        &grams,
        Some(cost.clone()),
        "Supplier",
        &format!("PO-{stamp}"),
    );
    let sig = d::sign_event(CHAIN_ID, desk_id, &m, &supplier)?;
    println!("{}", d::words(&book.record(m, Some(&sig))?));
    let after_buy = offset_and_read(&book, user, "the purchase").await?;

    println!("\n## 2. shipped (bill of lading signed by the carrier)");
    let bl = milestone(
        &deal,
        MilestoneKind::Shipped,
        &grams,
        None,
        "Carrier",
        &format!("BL-{stamp}"),
    );
    // A bill of lading for this deal signed by someone else is refused first.
    let forged = d::sign_event(CHAIN_ID, desk_id, &bl, &mallory)?;
    println!(
        "forged bill of lading (a stranger's key, naming the Carrier): {}",
        book.record(bl.clone(), Some(&forged)).unwrap_err()
    );
    let sig = d::sign_event(CHAIN_ID, desk_id, &bl, &carrier)?;
    println!("{}", d::words(&book.record(bl, Some(&sig))?));
    let in_transit = offset_and_read(&book, user, "shipping (no change in exposure)").await?;

    println!("\n## 3. statement for the lender, while the gold is at sea");
    let inputs = d::statement_inputs(Goods::Gold, user, stamp, now()).await?;
    let st = book.desk.statement(&deal, &inputs)?;
    let key = operator::statement::key()?;
    let signed = d::sign_statement(CHAIN_ID, &st, &key)?;
    println!("{}", serde_json::to_string_pretty(&signed)?);
    println!(
        "statement verifies, signer {:#x}",
        d::verify_statement(&signed)?
    );
    let mut tampered = signed.clone();
    tampered["statement"]["eligibleUsdc"] = serde_json::json!("1000000");
    println!(
        "a changed number: {}",
        d::verify_statement(&tampered).unwrap_err()
    );

    println!("\n## 4. the root on chain, and one deal disclosed against it");
    let root = book.desk.root()?;
    let anchor = Address::from_str(std::fs::read_to_string(ANCHOR_FILE)?.trim())?;
    let rpc = std::env::var("TAMEION_RPC_ARBITRUM_SEPOLIA")
        .unwrap_or_else(|_| "https://sepolia-rollup.arbitrum.io/rpc".into());
    let tk_path = std::env::var("TREASURY_KEY_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home(".tameion/treasury.key"));
    let tk = std::fs::read_to_string(&tk_path)?;
    let anchorer = PrivateKeySigner::from_str(tk.trim())?.address();
    let client = arc::ArcClient::with_signer(&rpc, tk.trim())?;
    drop(tk);
    let root_b = B256::from_str(&root.to_hex())?;
    let tx = client.anchor_root(anchor, root_b).await?;
    println!(
        "anchored root {} in tx {tx:#x} (https://sepolia.arbiscan.io/tx/{tx:#x})",
        root.to_hex()
    );
    let reader = arc::ArcClient::read_only(&rpc)?;
    println!(
        "rootOf({anchorer:#x}) on chain = {:#x}",
        reader.anchored_root(anchor, anchorer).await?
    );
    println!(
        "tx carries this root, from the anchorer, to the anchor: {}",
        reader.anchored_in(tx, anchor, anchorer, root_b).await?
    );
    let disc = book.desk.disclose(&deal)?;
    println!(
        "disclosure: {} of {} milestones, root {}",
        disc.items.len(),
        disc.leaves,
        disc.root
    );
    disc.verify()?;
    println!("disclosure verifies against the root: true");
    let mut bad = disc.clone();
    bad.items[0].canonical = bad.items[0].canonical.replace("kind=bought", "kind=sold");
    println!("a changed disclosed line: {}", bad.verify().unwrap_err());
    write_kit(
        &book_path,
        &signed,
        &disc,
        &d::signed_milestones(&book, &deal),
        &book.parties,
        tx,
        anchor,
        anchorer,
    )?;

    println!("\n## 5. arrived, then sold (signed by the carrier, then the buyer)");
    let an = milestone(
        &deal,
        MilestoneKind::Arrived,
        &grams,
        None,
        "Carrier",
        &format!("AN-{stamp}"),
    );
    let sig = d::sign_event(CHAIN_ID, desk_id, &an, &carrier)?;
    println!("{}", d::words(&book.record(an, Some(&sig))?));
    let too_much = milestone(
        &deal,
        MilestoneKind::Sold,
        "1000",
        Some("1".into()),
        "Buyer",
        "SO-X",
    );
    let sig = d::sign_event(CHAIN_ID, desk_id, &too_much, &buyer)?;
    println!(
        "a sale of more than the deal holds: {}",
        book.record(too_much, Some(&sig)).unwrap_err()
    );
    let mark2 = d::venue_mark("PAXG").await?;
    let so = milestone(
        &deal,
        MilestoneKind::Sold,
        &grams,
        Some(usd(oz * mark2 * Decimal::new(1015, 3))),
        "Buyer",
        &format!("SO-{stamp}"),
    );
    let sig = d::sign_event(CHAIN_ID, desk_id, &so, &buyer)?;
    println!("{}", d::words(&book.record(so, Some(&sig))?));
    let closed = offset_and_read(&book, user, "the sale").await?;

    println!("\n## 6. the books");
    for ccy in [ledger::Currency::GoldGram, ledger::Currency::Usdc] {
        let (dr, cr) = book.desk.journal(ccy).trial_balance(ccy)?;
        println!(
            "trial balance {ccy}: debits {dr} = credits {cr}: {}",
            dr == cr
        );
    }
    let reopened = Book::open(&book_path, CHAIN_ID, desk_id)?;
    println!(
        "book file replays with every signature re-checked: root {}",
        reopened.desk.root()?.to_hex()
    );
    println!(
        "\nsummary: short after purchase {after_buy}, in transit {in_transit}, after sale {closed}"
    );
    Ok(())
}

/// What a lender gets: the signed statement, the disclosure and where the
/// root was anchored, in one file `scripts/verify_desk.sh` checks.
#[allow(clippy::too_many_arguments)]
fn write_kit(
    book_path: &Path,
    signed: &serde_json::Value,
    disc: &desk::Disclosure,
    milestones: &[serde_json::Value],
    parties: &[d::Party],
    tx: B256,
    anchor: Address,
    anchorer: Address,
) -> Result<(), String> {
    let kit = serde_json::json!({
        "signed": signed,
        "disclosure": disc,
        "milestones": milestones,
        "parties": parties,
        "anchor": { "chainId": CHAIN_ID, "contract": format!("{anchor:#x}"), "tx": format!("{tx:#x}"), "from": format!("{anchorer:#x}") },
    });
    let path = book_path.with_extension("lender.json");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&kit).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    println!("lender kit: {}", path.display());
    Ok(())
}
