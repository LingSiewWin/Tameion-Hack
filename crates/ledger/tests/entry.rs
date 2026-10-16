//! The Canteen survey lists six errors that all pass a debits-equal-credits
//! check. These tests walk that list and are explicit about which ones this
//! layer catches and which ones it structurally cannot -- because claiming to
//! catch all six would be the same overreach the article is complaining about.

use ledger::{
    AccountId, Credit, Currency, Debit, DocumentKind, DocumentRef, Entry, Money, PostError,
};

fn usdc(s: &str) -> Money {
    Money::parse(s, Currency::Usdc).unwrap()
}

fn acct(s: &str) -> AccountId {
    AccountId::new(s)
}

fn doc() -> DocumentRef {
    DocumentRef::new(DocumentKind::Settlement, "0xdeadbeef")
}

#[test]
fn a_balanced_entry_posts() {
    let e = Entry::post(
        doc(),
        vec![Debit::new(acct("expense:saas"), usdc("20.000000")).unwrap()],
        vec![Credit::new(acct("asset:usdc"), usdc("20.000000")).unwrap()],
    )
    .unwrap();

    assert_eq!(e.total(), usdc("20.000000"));
    assert_eq!(e.currency(), Currency::Usdc);
    assert_eq!(e.document().id(), "0xdeadbeef");
}

#[test]
fn an_unbalanced_entry_does_not_become_a_value() {
    // ERPNext would let this through within its allowance and post the gap
    // to Round Off. Here there is no Entry to hold, only an error naming
    // the difference.
    let err = Entry::post(
        doc(),
        vec![Debit::new(acct("expense:saas"), usdc("20.000000")).unwrap()],
        vec![Credit::new(acct("asset:usdc"), usdc("19.999999")).unwrap()],
    )
    .unwrap_err();

    match err {
        PostError::Unbalanced { difference, .. } => {
            assert_eq!(difference, usdc("0.000001"));
            // One minor unit. Exactly the residue that fills a Round Off
            // account when six-decimal settlement meets a rounding rule.
        }
        other => panic!("expected Unbalanced, got {other:?}"),
    }
}

#[test]
fn the_half_unit_allowance_does_not_exist_here() {
    // ERPNext: "lets a Journal Entry or Payment Entry be out of balance by
    // five units at the last decimal place". Any other voucher may be out by
    // half a unit of currency. Both are accepted and the gap is booked.
    //
    // A thousand postings each off by one minor unit is a thousand Round Off
    // entries there, and a thousand refusals here.
    for _ in 0..1000 {
        let r = Entry::post(
            doc(),
            vec![Debit::new(acct("expense:saas"), usdc("20.000000")).unwrap()],
            vec![Credit::new(acct("asset:usdc"), usdc("19.999999")).unwrap()],
        );
        assert!(r.is_err(), "no allowance, however small");
    }
}

// ---- the six errors from the article ----

#[test]
fn complete_reversal_is_caught_by_the_type_system() {
    // "A complete reversal swaps the debit and the credit." Debits still
    // equal credits, so arithmetic cannot see it.
    //
    // Here Debit and Credit are distinct types, so the swap is a compile
    // error, not a runtime one. The line that would express the bug is:
    //
    //     Entry::post(doc(), vec![credit_leg], vec![debit_leg])
    //                        ^^^^^^^^^^^^^^^^ expected Vec<Debit>, found Vec<Credit>
    //
    // Proven as a compile_fail doctest in src/lib.rs. This test only pins
    // the fact that the two types are not interchangeable at runtime either.
    let d = Debit::new(acct("a"), usdc("1.000000")).unwrap();
    let c = Credit::new(acct("a"), usdc("1.000000")).unwrap();
    assert_eq!(d.amount(), c.amount());
    assert_eq!(d.to_string(), "debit a 1.000000 USDC");
    assert_eq!(c.to_string(), "credit a 1.000000 USDC");
}

#[test]
fn error_of_original_entry_is_caught_when_it_is_one_sided() {
    // "the wrong amount on both sides" passes -- see the next test. The
    // wrong amount on ONE side does not.
    let err = Entry::post(
        doc(),
        vec![Debit::new(acct("expense:saas"), usdc("200.000000")).unwrap()],
        vec![Credit::new(acct("asset:usdc"), usdc("20.000000")).unwrap()],
    )
    .unwrap_err();
    assert!(matches!(err, PostError::Unbalanced { .. }));
}

#[test]
fn error_of_original_entry_on_both_sides_is_not_caught_here_by_design() {
    // The invoice said 20 USDC. Someone read it as 200 and booked 200 on
    // both sides. This is balanced, and no ledger can know otherwise: the
    // check that catches it is the three-way match against the document,
    // which lives a layer up.
    let e = Entry::post(
        doc(),
        vec![Debit::new(acct("expense:saas"), usdc("200.000000")).unwrap()],
        vec![Credit::new(acct("asset:usdc"), usdc("200.000000")).unwrap()],
    )
    .unwrap();
    assert_eq!(e.total(), usdc("200.000000"));
    // Posted happily. What the entry DOES carry is the document it claims to
    // book, so the layer above has something to compare against.
    assert_eq!(e.document().kind(), DocumentKind::Settlement);
}

#[test]
fn error_of_commission_is_not_caught_here_and_cannot_be() {
    // "the right amount in the wrong account of the same kind, such as a
    // payment posted to the wrong vendor". Both accounts are expenses, the
    // amount is right, the entry balances.
    //
    // The article is explicit that reconciliation cannot catch this either:
    // the bank confirms you paid exactly whom you told it to pay. The
    // control that catches it is change control on the vendor master.
    let e = Entry::post(
        doc(),
        vec![Debit::new(acct("expense:vendor-WRONG"), usdc("20.000000")).unwrap()],
        vec![Credit::new(acct("asset:usdc"), usdc("20.000000")).unwrap()],
    )
    .unwrap();
    assert_eq!(e.total(), usdc("20.000000"));
}

#[test]
fn a_document_is_mandatory_so_the_upper_layers_have_an_anchor() {
    // Firefly III and Invoice Ninja keep a running balance beside the
    // invoice, so there is nothing behind an entry to match against and the
    // three-way match has nowhere to attach. Entry::post takes a
    // DocumentRef by value; there is no constructor without one.
    let e = Entry::post(
        DocumentRef::new(DocumentKind::Obligation, "0x1111@1984916082"),
        vec![Debit::new(acct("expense:saas"), usdc("20.000000")).unwrap()],
        vec![Credit::new(acct("asset:usdc"), usdc("20.000000")).unwrap()],
    )
    .unwrap();
    assert_eq!(e.document().to_string(), "obligation:0x1111@1984916082");
}

// ---- leg hygiene ----

#[test]
fn a_negative_leg_is_refused() {
    // In double entry a reduction is the other side, not a minus sign.
    // Allowing negatives means a "debit" can secretly be a credit.
    let err = Debit::new(acct("asset:usdc"), usdc("-5.000000")).unwrap_err();
    assert!(matches!(err, PostError::NegativeLeg { .. }));
    assert!(err.to_string().contains("post to the other side instead"));
}

#[test]
fn a_zero_leg_is_refused() {
    let err = Credit::new(acct("asset:usdc"), usdc("0.000000")).unwrap_err();
    assert!(matches!(err, PostError::ZeroLeg { .. }));
}

#[test]
fn an_entry_needs_both_sides() {
    let only_debits = Entry::post(
        doc(),
        vec![Debit::new(acct("expense:saas"), usdc("20.000000")).unwrap()],
        vec![],
    )
    .unwrap_err();
    assert!(matches!(only_debits, PostError::Empty { side: "credit" }));
}

#[test]
fn currencies_cannot_be_mixed_inside_an_entry() {
    let err = Entry::post(
        doc(),
        vec![Debit::new(acct("expense:saas"), usdc("20.000000")).unwrap()],
        vec![
            Credit::new(
                acct("asset:usd"),
                Money::parse("20.00", Currency::Usd).unwrap(),
            )
            .unwrap(),
        ],
    )
    .unwrap_err();
    assert!(matches!(err, PostError::MixedCurrency { .. }));
}

#[test]
fn many_legs_on_each_side_still_have_to_balance() {
    // One payment across several obligations, the Invoice Ninja Paymentable
    // shape, but balanced.
    let e = Entry::post(
        doc(),
        vec![
            Debit::new(acct("expense:saas"), usdc("20.000000")).unwrap(),
            Debit::new(acct("expense:contractor"), usdc("35.000000")).unwrap(),
        ],
        vec![Credit::new(acct("asset:usdc"), usdc("55.000000")).unwrap()],
    )
    .unwrap();
    assert_eq!(e.total(), usdc("55.000000"));
    assert_eq!(e.debits().len(), 2);
    assert_eq!(e.credits().len(), 1);
}
