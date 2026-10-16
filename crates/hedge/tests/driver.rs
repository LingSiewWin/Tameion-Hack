use hedge::driver::parse_holdings;
use rust_decimal::dec;

#[test]
fn holdings_parse_and_upper_case() {
    let h = parse_holdings(" eth=0.5, SOL=20 ,").unwrap();
    assert_eq!(h.len(), 2);
    assert_eq!(h[0].coin, "ETH");
    assert_eq!(h[0].size, dec!(0.5));
    assert_eq!(h[1].coin, "SOL");
    assert_eq!(h[1].size, dec!(20));
}

#[test]
fn malformed_holdings_are_refused_with_the_offending_piece() {
    let e = parse_holdings("ETH").unwrap_err();
    assert!(e.contains("COIN=SIZE"), "{e}");
    let e = parse_holdings("ETH=lots").unwrap_err();
    assert!(e.contains("lots"), "{e}");
    assert!(parse_holdings("").unwrap().is_empty());
}
