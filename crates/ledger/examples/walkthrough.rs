//! Replays a treasury day through the ledger and shows what it refuses.
//! Run with:  cargo run --example walkthrough

use ledger::{
    Account, AccountId, AccountKind, BalanceAssertion, Chart, Credit, Currency, Debit,
    DocumentKind, DocumentRef, Entry, Journal, Money,
};

fn usdc(s: &str) -> Money {
    Money::parse(s, Currency::Usdc).unwrap()
}
fn acct(s: &str) -> AccountId {
    AccountId::new(s)
}
fn rule(title: &str) {
    println!("\n\x1b[1m{title}\x1b[0m\n{}", "-".repeat(title.len()));
}

fn main() {
    let chart = Chart::new()
        .with(Account::new(
            acct("asset:usdc"),
            AccountKind::Asset,
            Currency::Usdc,
        ))
        .with(Account::new(
            acct("expense:saas"),
            AccountKind::Expense,
            Currency::Usdc,
        ))
        .with(Account::new(
            acct("expense:contractor"),
            AccountKind::Expense,
            Currency::Usdc,
        ))
        .with(Account::new(
            acct("revenue:sales"),
            AccountKind::Revenue,
            Currency::Usdc,
        ));

    let mut journal = Journal::new();

    rule("1. Revenue arrives");
    let funding = Entry::post(
        DocumentRef::new(DocumentKind::Settlement, "0xfund"),
        vec![Debit::new(acct("asset:usdc"), usdc("100.000000")).unwrap()],
        vec![Credit::new(acct("revenue:sales"), usdc("100.000000")).unwrap()],
    )
    .unwrap();
    print!("{funding}");
    journal.append(funding);

    rule("2. The agent settles two obligations");
    for (account, amount, doc) in [
        ("expense:saas", "20.000000", "0x31039be4"),
        ("expense:contractor", "35.000000", "0x8f2a01cd"),
    ] {
        let e = Entry::post(
            DocumentRef::new(DocumentKind::Settlement, doc),
            vec![Debit::new(acct(account), usdc(amount)).unwrap()],
            vec![Credit::new(acct("asset:usdc"), usdc(amount)).unwrap()],
        )
        .unwrap();
        print!("{e}");
        journal.append(e);
    }

    rule("3. Balances");
    for id in chart.ids() {
        println!(
            "  {:<22} {}",
            id.to_string(),
            journal.balance(&chart, id).unwrap()
        );
    }
    let (d, c) = journal.trial_balance(Currency::Usdc).unwrap();
    println!("\n  trial balance: debits {d} / credits {c}");
    println!("  closes: {}", d == c);

    rule("4. The ERPNext scenario: one minor unit short");
    println!("  ERPNext books a gap this size to a Round Off account and lets");
    println!("  the entry through. Six-decimal settlement makes that residue");
    println!("  arrive on every posting. Here:\n");
    match Entry::post(
        DocumentRef::new(DocumentKind::Settlement, "0xshort"),
        vec![Debit::new(acct("expense:saas"), usdc("20.000000")).unwrap()],
        vec![Credit::new(acct("asset:usdc"), usdc("19.999999")).unwrap()],
    ) {
        Ok(_) => println!("  posted (this would be the bug)"),
        Err(e) => println!("  \x1b[31mREFUSED\x1b[0m  {e}"),
    }

    rule("5. A negative leg");
    println!("  A reduction is the other side, not a minus sign.\n");
    match Debit::new(acct("asset:usdc"), usdc("-5.000000")) {
        Ok(_) => println!("  accepted"),
        Err(e) => println!("  \x1b[31mREFUSED\x1b[0m  {e}"),
    }

    rule("6. Balance assertions");
    let exact = BalanceAssertion::exact(acct("asset:usdc"), usdc("45.000000"));
    match exact.verify(&journal, &chart) {
        Ok(v) => println!("  \x1b[32mHOLDS\x1b[0m    asset:usdc == {v}"),
        Err(e) => println!("  FAILED   {e}"),
    }
    let wrong = BalanceAssertion::exact(acct("asset:usdc"), usdc("45.000001"));
    match wrong.verify(&journal, &chart) {
        Ok(v) => println!("  holds    {v}"),
        Err(e) => println!("  \x1b[31mFAILS\x1b[0m    {e}"),
    }
    let lenient =
        BalanceAssertion::within(acct("asset:usdc"), usdc("45.000001"), usdc("0.000001")).unwrap();
    match lenient.verify(&journal, &chart) {
        Ok(v) => println!("  \x1b[32mHOLDS\x1b[0m    with tolerance 0.000001 written down: {v}"),
        Err(e) => println!("  fails    {e}"),
    }

    rule("7. A mistake, and its correction");
    println!("  There is no remove(). A correction is a new entry.\n");
    let oops = Entry::post(
        DocumentRef::new(DocumentKind::Settlement, "0xwrong-vendor"),
        vec![Debit::new(acct("expense:saas"), usdc("10.000000")).unwrap()],
        vec![Credit::new(acct("asset:usdc"), usdc("10.000000")).unwrap()],
    )
    .unwrap();
    journal.append(oops);
    let reversal = Entry::post(
        DocumentRef::new(DocumentKind::Adjustment, "reverses:0xwrong-vendor"),
        vec![Debit::new(acct("asset:usdc"), usdc("10.000000")).unwrap()],
        vec![Credit::new(acct("expense:saas"), usdc("10.000000")).unwrap()],
    )
    .unwrap();
    journal.append(reversal);
    println!(
        "  entries in journal: {}   asset:usdc back to {}",
        journal.len(),
        journal.balance(&chart, &acct("asset:usdc")).unwrap()
    );
    println!("  both the mistake and its reversal stay in the record.");
    println!();
}
