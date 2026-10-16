use ledger::{DecisionLog, EventBody, Timestamp};
use operator::hedge_advice::{Advice, event_for, parse_proposals};
use operator::reasoner::ReasonerError;

#[test]
fn a_well_formed_reply_parses() {
    let text = r#"{"proposals":[
        {"coin":"ETH","advice":"widen-band","band_bps":800,"reason":"Funding has been negative for a week; rebalancing less often saves carry."},
        {"coin":"SOL","advice":"keep","band_bps":null,"reason":"The numbers give no reason to change."}
    ]}"#;
    let ps = parse_proposals(text, &["ETH", "SOL"], 500).unwrap();
    assert_eq!(ps.len(), 2);
    assert_eq!(ps[0].advice, Advice::WidenBand);
    assert_eq!(ps[0].band_bps, Some(800));
    assert_eq!(ps[1].advice, Advice::Keep);
    assert_eq!(ps[1].band_bps, None);
}

#[test]
fn population_is_enforced() {
    let missing = r#"{"proposals":[{"coin":"ETH","advice":"keep","band_bps":null,"reason":"x"}]}"#;
    assert!(matches!(
        parse_proposals(missing, &["ETH", "SOL"], 500),
        Err(ReasonerError::WrongPopulation)
    ));
    let extra = r#"{"proposals":[{"coin":"ETH","advice":"keep","band_bps":null,"reason":"x"},{"coin":"DOGE","advice":"keep","band_bps":null,"reason":"x"}]}"#;
    assert!(matches!(
        parse_proposals(extra, &["ETH", "SOL"], 500),
        Err(ReasonerError::WrongPopulation)
    ));
    let dup = r#"{"proposals":[{"coin":"ETH","advice":"keep","band_bps":null,"reason":"x"},{"coin":"ETH","advice":"keep","band_bps":null,"reason":"x"}]}"#;
    assert!(matches!(
        parse_proposals(dup, &["ETH", "SOL"], 500),
        Err(ReasonerError::WrongPopulation)
    ));
}

#[test]
fn band_rules_are_enforced_not_hoped_for() {
    // tighten must go below the current band
    let t = r#"{"proposals":[{"coin":"ETH","advice":"tighten-band","band_bps":700,"reason":"x"}]}"#;
    assert!(matches!(
        parse_proposals(t, &["ETH"], 500),
        Err(ReasonerError::Unusable(_))
    ));
    // widen must go above it
    let w = r#"{"proposals":[{"coin":"ETH","advice":"widen-band","band_bps":300,"reason":"x"}]}"#;
    assert!(matches!(
        parse_proposals(w, &["ETH"], 500),
        Err(ReasonerError::Unusable(_))
    ));
    // and stay within 100..=5000
    let big =
        r#"{"proposals":[{"coin":"ETH","advice":"widen-band","band_bps":9000,"reason":"x"}]}"#;
    assert!(matches!(
        parse_proposals(big, &["ETH"], 500),
        Err(ReasonerError::Unusable(_))
    ));
    // keep with a band is confused; refused
    let k = r#"{"proposals":[{"coin":"ETH","advice":"keep","band_bps":400,"reason":"x"}]}"#;
    assert_eq!(parse_proposals(k, &["ETH"], 500).unwrap()[0].band_bps, None);
    // unknown advice
    let u = r#"{"proposals":[{"coin":"ETH","advice":"yolo","band_bps":null,"reason":"x"}]}"#;
    assert!(matches!(
        parse_proposals(u, &["ETH"], 500),
        Err(ReasonerError::Unusable(_))
    ));
    // empty reason
    let e = r#"{"proposals":[{"coin":"ETH","advice":"keep","band_bps":null,"reason":"  "}]}"#;
    assert!(matches!(
        parse_proposals(e, &["ETH"], 500),
        Err(ReasonerError::Unusable(_))
    ));
}

#[test]
fn a_proposal_is_a_typed_attributed_event_that_changes_the_root() {
    let text = r#"{"proposals":[{"coin":"ETH","advice":"pause","band_bps":null,"reason":"Every pass refuses for lack of collateral; retrying is noise."}]}"#;
    let p = parse_proposals(text, &["ETH"], 500).unwrap().remove(0);
    let body = event_for(&p, "claude-opus-5");
    assert_eq!(body.kind(), "PROPOSAL");
    assert!(
        body.note()
            .starts_with("claude-opus-5 proposes pause for ETH")
    );
    assert!(body.note().contains("nothing changed"));
    match &body {
        EventBody::HedgeProposal {
            by,
            advice,
            band_bps,
            ..
        } => {
            assert_eq!(by, "claude-opus-5");
            assert_eq!(advice, "pause");
            assert!(band_bps.is_none());
        }
        other => panic!("{other:?}"),
    }
    let at = Timestamp::from_unix(1_800_000_000);
    let mut a = DecisionLog::new();
    a.append(at, body.clone());
    let mut b = DecisionLog::new();
    b.append(at, body);
    assert_eq!(a.root(), b.root());
    let mut c = DecisionLog::new();
    c.append(at, event_for(&p, "someone-else"));
    assert_ne!(a.root(), c.root(), "attribution is part of the record");
}
