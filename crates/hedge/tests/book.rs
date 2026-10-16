use hedge::book::{self, accounts, chart, net_view, usdc};
use ledger::{Currency, Journal, Money};
use rust_decimal::dec;

#[test]
fn usdc_is_exact_at_six_places_and_rounds_half_even_beyond() {
    assert_eq!(
        usdc(dec!(20.000000)).unwrap(),
        Money::minor(20_000_000, Currency::Usdc)
    );
    assert_eq!(
        usdc(dec!(-1.234567)).unwrap(),
        Money::minor(-1_234_567, Currency::Usdc)
    );
    // 0.0000005 is a tie: half-to-even rounds to 0.000000.
    assert_eq!(usdc(dec!(0.0000005)).unwrap(), Money::zero(Currency::Usdc));
    // 0.0000015 ties to 0.000002.
    assert_eq!(
        usdc(dec!(0.0000015)).unwrap(),
        Money::minor(2, Currency::Usdc)
    );
}

#[test]
fn a_hedged_move_nets_to_zero_in_the_books() {
    let chart = chart(&["ETH"]);
    let mut j = Journal::new();
    // ETH falls: the holding loses 60, the short gains 60.
    j.append(book::mark_spot("ETH", dec!(-60), "ETH@1").unwrap().unwrap());
    j.append(book::mark_hedge("ETH", dec!(60), "ETH@1").unwrap().unwrap());
    let v = net_view(&j, &chart, "ETH").unwrap();
    assert_eq!(v.spot, Money::minor(-60_000_000, Currency::Usdc));
    assert_eq!(v.hedge, Money::minor(60_000_000, Currency::Usdc));
    assert_eq!(v.net, Money::zero(Currency::Usdc));
    let (d, c) = j.trial_balance(Currency::Usdc).unwrap();
    assert_eq!(d, c);
    // Spot loss is an expense; hedge gain is revenue. They cancel in P&L too.
    assert_eq!(
        j.balance(&chart, &accounts::spot_loss()).unwrap(),
        j.balance(&chart, &accounts::hedge_gain()).unwrap()
    );
}

#[test]
fn margin_deposit_moves_usdc_between_two_assets() {
    let chart = chart(&[]);
    let mut j = Journal::new();
    j.append(book::deposit_margin(dec!(500), "fund-1").unwrap());
    assert_eq!(
        j.balance(&chart, &accounts::hedge_margin()).unwrap(),
        Money::minor(500_000_000, Currency::Usdc)
    );
    assert_eq!(
        j.balance(&chart, &accounts::treasury_usdc()).unwrap(),
        Money::minor(-500_000_000, Currency::Usdc)
    );
}

#[test]
fn funding_received_and_paid_land_on_opposite_sides() {
    let chart = chart(&["BTC"]);
    let mut j = Journal::new();
    j.append(book::funding("BTC", dec!(0.75), "h1").unwrap().unwrap());
    j.append(book::funding("BTC", dec!(-0.25), "h2").unwrap().unwrap());
    assert_eq!(
        j.balance(&chart, &accounts::hedge_margin()).unwrap(),
        Money::minor(500_000, Currency::Usdc)
    );
    assert_eq!(
        j.balance(&chart, &accounts::funding_income()).unwrap(),
        Money::minor(750_000, Currency::Usdc)
    );
    assert_eq!(
        j.balance(&chart, &accounts::funding_expense()).unwrap(),
        Money::minor(250_000, Currency::Usdc)
    );
}

#[test]
fn zero_and_sub_unit_deltas_book_nothing() {
    assert!(book::mark_spot("ETH", dec!(0), "x").unwrap().is_none());
    assert!(
        book::mark_hedge("ETH", dec!(0.0000001), "x")
            .unwrap()
            .is_none()
    );
    assert!(book::funding("ETH", dec!(0), "x").unwrap().is_none());
    assert!(book::fee(dec!(0), "x").unwrap().is_none());
}

#[test]
fn unknown_coin_is_refused_by_the_chart() {
    let chart = chart(&["ETH"]);
    let mut j = Journal::new();
    j.append(book::mark_spot("SOL", dec!(1), "SOL@1").unwrap().unwrap());
    assert!(
        net_view(&j, &chart, "SOL").is_err(),
        "SOL is not in the chart"
    );
}
