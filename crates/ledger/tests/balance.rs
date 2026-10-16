//! Balances, and the difference between a tolerance you declared and an
//! allowance you inherited.

use ledger::{
    Account, AccountId, AccountKind, AssertionFailed, BalanceAssertion, Chart, Credit, Currency,
    Debit, DocumentKind, DocumentRef, Entry, Journal, Money,
};

fn usdc(s: &str) -> Money {
    Money::parse(s, Currency::Usdc).unwrap()
}

fn acct(s: &str) -> AccountId {
    AccountId::new(s)
}

/// Treasury holding USDC, one expense account per counterparty.
fn chart() -> Chart {
    Chart::new()
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
        ))
}

fn pay(from: &str, to: &str, amount: &str, doc_id: &str) -> Entry {
    Entry::post(
        DocumentRef::new(DocumentKind::Settlement, doc_id),
        vec![Debit::new(acct(to), usdc(amount)).unwrap()],
        vec![Credit::new(acct(from), usdc(amount)).unwrap()],
    )
    .unwrap()
}

#[test]
fn funding_then_two_payments_leaves_the_right_balance() {
    let chart = chart();
    let mut j = Journal::new();

    // Revenue arrives: asset up (debit), revenue up (credit).
    j.append(
        Entry::post(
            DocumentRef::new(DocumentKind::Settlement, "0xfund"),
            vec![Debit::new(acct("asset:usdc"), usdc("100.000000")).unwrap()],
            vec![Credit::new(acct("revenue:sales"), usdc("100.000000")).unwrap()],
        )
        .unwrap(),
    );
    j.append(pay("asset:usdc", "expense:saas", "20.000000", "0xaaa"));
    j.append(pay(
        "asset:usdc",
        "expense:contractor",
        "35.000000",
        "0xbbb",
    ));

    assert_eq!(
        j.balance(&chart, &acct("asset:usdc")).unwrap(),
        usdc("45.000000")
    );
    assert_eq!(
        j.balance(&chart, &acct("expense:saas")).unwrap(),
        usdc("20.000000")
    );
    // Revenue is a credit-normal account, so a positive balance means
    // revenue earned rather than a debit surplus.
    assert_eq!(
        j.balance(&chart, &acct("revenue:sales")).unwrap(),
        usdc("100.000000")
    );
}

#[test]
fn the_trial_balance_closes_because_every_entry_had_to() {
    let chart = chart();
    let mut j = Journal::new();
    j.append(
        Entry::post(
            DocumentRef::new(DocumentKind::Settlement, "0xfund"),
            vec![Debit::new(acct("asset:usdc"), usdc("100.000000")).unwrap()],
            vec![Credit::new(acct("revenue:sales"), usdc("100.000000")).unwrap()],
        )
        .unwrap(),
    );
    for i in 0..500 {
        j.append(pay(
            "asset:usdc",
            "expense:saas",
            "0.000001",
            &format!("0x{i}"),
        ));
    }

    let (debits, credits) = j.trial_balance(Currency::Usdc).unwrap();
    assert_eq!(debits, credits, "no Round Off account needed to close this");
    assert_eq!(
        j.balance(&chart, &acct("expense:saas")).unwrap(),
        usdc("0.000500")
    );
    assert_eq!(
        j.balance(&chart, &acct("asset:usdc")).unwrap(),
        usdc("99.999500")
    );
}

#[test]
fn an_exact_assertion_tolerates_nothing() {
    let chart = chart();
    let mut j = Journal::new();
    j.append(
        Entry::post(
            DocumentRef::new(DocumentKind::Settlement, "0xfund"),
            vec![Debit::new(acct("asset:usdc"), usdc("20.000000")).unwrap()],
            vec![Credit::new(acct("revenue:sales"), usdc("20.000000")).unwrap()],
        )
        .unwrap(),
    );

    BalanceAssertion::exact(acct("asset:usdc"), usdc("20.000000"))
        .verify(&j, &chart)
        .expect("exact match holds");

    // One minor unit out. ERPNext would absorb this into Round Off.
    let err = BalanceAssertion::exact(acct("asset:usdc"), usdc("19.999999"))
        .verify(&j, &chart)
        .unwrap_err();

    match err {
        AssertionFailed::OutOfTolerance(b) => {
            assert_eq!(b.difference, usdc("0.000001"));
            assert_eq!(b.tolerance, usdc("0.000000"));
        }
        other => panic!("expected OutOfTolerance, got {other:?}"),
    }
}

#[test]
fn tolerance_has_to_be_written_down_to_apply() {
    let chart = chart();
    let mut j = Journal::new();
    j.append(
        Entry::post(
            DocumentRef::new(DocumentKind::Settlement, "0xfund"),
            vec![Debit::new(acct("asset:usdc"), usdc("20.000000")).unwrap()],
            vec![Credit::new(acct("revenue:sales"), usdc("20.000000")).unwrap()],
        )
        .unwrap(),
    );

    // The author states the slack, in the currency, at the call site.
    let lenient =
        BalanceAssertion::within(acct("asset:usdc"), usdc("19.999999"), usdc("0.000001")).unwrap();
    assert_eq!(lenient.verify(&j, &chart).unwrap(), usdc("20.000000"));

    // And it applies only as far as it was written.
    let still_strict =
        BalanceAssertion::within(acct("asset:usdc"), usdc("19.999998"), usdc("0.000001")).unwrap();
    assert!(still_strict.verify(&j, &chart).is_err());
}

#[test]
fn the_error_message_names_the_gap() {
    let chart = chart();
    let mut j = Journal::new();
    j.append(
        Entry::post(
            DocumentRef::new(DocumentKind::Settlement, "0xfund"),
            vec![Debit::new(acct("asset:usdc"), usdc("20.000000")).unwrap()],
            vec![Credit::new(acct("revenue:sales"), usdc("20.000000")).unwrap()],
        )
        .unwrap(),
    );
    let err = BalanceAssertion::exact(acct("asset:usdc"), usdc("19.500000"))
        .verify(&j, &chart)
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("expected 19.500000 USDC"), "{msg}");
    assert!(msg.contains("found 20.000000 USDC"), "{msg}");
    assert!(msg.contains("off by 0.500000 USDC"), "{msg}");
}

#[test]
fn an_unknown_account_is_refused_not_created() {
    let chart = chart();
    let j = Journal::new();
    let err = j
        .balance(&chart, &acct("expense:invented-from-a-pdf"))
        .unwrap_err();
    assert!(err.to_string().contains("is not in the chart"));
}

#[test]
fn the_journal_has_no_way_to_remove_an_entry() {
    // Kill Bill refuses to change a COMMITTED invoice; corrections are new
    // adjustment items. Firefly III lets a rule delete a journal outright.
    // There is no `remove` here -- a reversal is posted, and both remain.
    let chart = chart();
    let mut j = Journal::new();
    j.append(pay("asset:usdc", "expense:saas", "20.000000", "0xoops"));

    // Reverse it by posting the mirror, as an adjustment.
    j.append(
        Entry::post(
            DocumentRef::new(DocumentKind::Adjustment, "reverses:0xoops"),
            vec![Debit::new(acct("asset:usdc"), usdc("20.000000")).unwrap()],
            vec![Credit::new(acct("expense:saas"), usdc("20.000000")).unwrap()],
        )
        .unwrap(),
    );

    assert_eq!(
        j.balance(&chart, &acct("expense:saas")).unwrap(),
        usdc("0.000000")
    );
    assert_eq!(
        j.len(),
        2,
        "both the mistake and its reversal stay in the record"
    );
}
