//! These tests are written as the four defects named in the Canteen survey,
//! plus the arithmetic laws money has to obey.

use ledger::{Currency, Money, MoneyError};

#[test]
fn invoice_ninja_cast_has_no_syntax() {
    // The bug: EUR 0.10 went through an integer cast, became 0, and the
    // subscription was marked paid with no payment.
    //
    // Here 0.10 EUR is 10 minor units and stays 10. There is no From<f64>,
    // no Into<f64>, and no deref to a primitive, so the cast that lost the
    // money cannot be written at all -- the line below would not compile:
    //
    //     let lost: i64 = Money::parse("0.10", Currency::Eur).unwrap() as i64;
    let dime = Money::parse("0.10", Currency::Eur).unwrap();
    assert_eq!(dime.to_minor(), 10);
    assert!(!dime.is_zero(), "0.10 EUR must never read as zero");
}

#[test]
fn six_decimal_settlement_survives_a_two_decimal_world() {
    // ERPNext allows a voucher to be out by half a unit of currency and
    // books the difference to Round Off. Feed it six-decimal settlement and
    // every posting leaves residue.
    let dust = Money::parse("0.000001", Currency::Usdc).unwrap();
    assert_eq!(dust.to_minor(), 1);
    assert_eq!(dust.to_string(), "0.000001 USDC");

    // A thousand of them add up to exactly what they should, no residue
    // account required.
    let thousand = Money::sum(std::iter::repeat_n(dust, 1000), Currency::Usdc).unwrap();
    assert_eq!(thousand.to_minor(), 1000);
    assert_eq!(thousand.to_string(), "0.001000 USDC");
}

#[test]
fn refuses_precision_it_cannot_hold_instead_of_rounding() {
    // ERPNext rounds and posts the difference. This refuses and says why,
    // so the caller decides.
    let err = Money::parse("0.1234567", Currency::Usdc).unwrap_err();
    assert_eq!(err, MoneyError::TooPrecise { got: 7, max: 6 });

    // Trailing zeros carry no value, so they are accepted.
    let fine = Money::parse("20.00000000", Currency::Usdc).unwrap();
    assert_eq!(fine.to_minor(), 20_000_000);
}

#[test]
fn gnucash_ceiling_is_not_reachable() {
    // GnuCash caps a commodity fraction at 10^9 because 10^10 would have
    // needed a wider integer. i128 at six decimals holds ~1.7e32 USDC.
    let huge = Money::parse("1000000000000.000000", Currency::Usdc).unwrap();
    assert_eq!(huge.to_minor(), 1_000_000_000_000_000_000);
    assert_eq!(huge.to_string(), "1000000000000.000000 USDC");
}

#[test]
fn currencies_do_not_mix() {
    let usdc = Money::parse("10.00", Currency::Usdc).unwrap();
    let usd = Money::parse("10.00", Currency::Usd).unwrap();
    assert_eq!(
        usdc.checked_add(usd).unwrap_err(),
        MoneyError::CurrencyMismatch {
            left: Currency::Usdc,
            right: Currency::Usd
        }
    );
    // Same face value, different currencies, so not equal either.
    assert_ne!(usdc, usd);
}

#[test]
fn overflow_is_refused_not_wrapped() {
    let max = Money::minor(i128::MAX, Currency::Usdc);
    let one = Money::minor(1, Currency::Usdc);
    assert_eq!(max.checked_add(one).unwrap_err(), MoneyError::Overflow);

    let min = Money::minor(i128::MIN, Currency::Usdc);
    assert_eq!(min.negate().unwrap_err(), MoneyError::Overflow);
}

#[test]
fn display_never_panics_at_the_boundaries() {
    // i128::MIN has no positive counterpart, the classic place a naive abs()
    // panics. Display works in unsigned space to avoid it.
    let min = Money::minor(i128::MIN, Currency::Usdc);
    assert!(min.to_string().starts_with('-'));
    let max = Money::minor(i128::MAX, Currency::Usdc);
    assert!(!max.to_string().starts_with('-'));
}

#[test]
fn parse_and_display_round_trip() {
    for s in ["0.000000", "1.000000", "20.500000", "-3.141593", "0.000001"] {
        let m = Money::parse(s, Currency::Usdc).unwrap();
        assert_eq!(
            m.to_string(),
            format!("{s} USDC"),
            "round trip failed for {s}"
        );
    }
}

#[test]
fn addition_is_associative_and_commutative() {
    let a = Money::parse("0.000001", Currency::Usdc).unwrap();
    let b = Money::parse("20.000000", Currency::Usdc).unwrap();
    let c = Money::parse("-3.500000", Currency::Usdc).unwrap();

    let left = a.checked_add(b).unwrap().checked_add(c).unwrap();
    let right = a.checked_add(b.checked_add(c).unwrap()).unwrap();
    assert_eq!(left, right, "associativity -- this is what floats break");

    assert_eq!(a.checked_add(b).unwrap(), b.checked_add(a).unwrap());
}

#[test]
fn subtraction_inverts_addition_exactly() {
    // The float version of this test fails: 0.1 + 0.2 - 0.2 != 0.1
    let a = Money::parse("0.100000", Currency::Usdc).unwrap();
    let b = Money::parse("0.200000", Currency::Usdc).unwrap();
    assert_eq!(a.checked_add(b).unwrap().checked_sub(b).unwrap(), a);
}

#[test]
fn malformed_input_is_rejected() {
    for bad in ["", "-", "abc", "1.2.3", "1,000.00", "1 000"] {
        assert!(
            Money::parse(bad, Currency::Usdc).is_err(),
            "{bad:?} should not parse"
        );
    }
}
