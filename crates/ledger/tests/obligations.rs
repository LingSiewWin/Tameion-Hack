//! Obligation detection: worked examples, edge cases, and properties.

use ledger::obligations::{MAX_INTERVAL_CV, MIN_SAMPLES};
use ledger::{
    Confidence, Counterparty, Currency, DetectionConfig, Direction, Money, ObligationKind, Seconds,
    Timestamp, Transfer, detect, total_due, upcoming,
};
use proptest::prelude::*;

const T0: i64 = 1_700_000_000;

fn usdc(s: &str) -> Money {
    Money::parse(s, Currency::Usdc).unwrap()
}

/// `n` payments to `who`, every `every_days`, each of `amount`.
fn schedule(who: &str, amount: &str, every_days: i64, n: usize) -> Vec<Transfer> {
    (0..n)
        .map(|i| Transfer {
            tx_hash: format!("0x{who}{i:04}"),
            counterparty: Counterparty::new(who),
            amount: usdc(amount),
            at: Timestamp::from_unix(T0 + (i as i64) * every_days * 86_400),
            direction: Direction::Out,
        })
        .collect()
}

// ------------------------------------------------------------------ basics

#[test]
fn a_clean_monthly_subscription_is_recovered() {
    let txs = schedule("0x1111", "20.000000", 30, 6);
    let found = detect(&txs, DetectionConfig::default());

    assert_eq!(found.len(), 1);
    let o = &found[0];
    assert_eq!(o.counterparty.as_str(), "0x1111");
    assert_eq!(o.kind, ObligationKind::Subscription);
    assert_eq!(o.typical_amount, usdc("20.000000"));
    assert_eq!(o.interval, Seconds::days(30).unwrap());
    assert_eq!(o.sample_count, 6);
    assert_eq!(o.interval_cv, 0.0, "a perfect schedule has zero variation");
    assert_eq!(o.amount_cv, 0.0);
    assert_eq!(o.evidence.len(), 6, "every payment is cited");
    // n=6 -> 1 - 0.75/5 = 0.85, with both factors at 1.0
    assert!((o.confidence.get() - 0.85).abs() < 1e-9, "{}", o.confidence);
}

#[test]
fn the_next_due_date_follows_the_last_payment() {
    let txs = schedule("0x1111", "20.000000", 30, 6);
    let o = &detect(&txs, DetectionConfig::default())[0];
    assert_eq!(o.last_paid, Timestamp::from_unix(T0 + 5 * 30 * 86_400));
    assert_eq!(o.next_due, Timestamp::from_unix(T0 + 6 * 30 * 86_400));
    assert!(!o.is_due(o.next_due - Seconds::DAY, Seconds::ZERO));
    assert!(o.is_due(o.next_due, Seconds::ZERO));
    assert!(
        o.is_due(o.next_due - Seconds::DAY, Seconds::DAY),
        "grace applies"
    );
}

#[test]
fn a_varying_amount_reads_as_a_recurring_payment_not_a_subscription() {
    let mut txs = schedule("0xC0DE", "1000.000000", 14, 6);
    for (i, t) in txs.iter_mut().enumerate() {
        // Contractor billing hours: same cadence, different totals.
        t.amount = usdc(&format!("{}.000000", 800 + i * 100));
    }
    let o = &detect(&txs, DetectionConfig::default())[0];
    assert_eq!(o.kind, ObligationKind::RecurringPayment);
    assert!(o.amount_cv > 0.02, "amount_cv {}", o.amount_cv);
    // Lower median of 800..1300 is 1000, an amount actually paid.
    assert_eq!(o.typical_amount, usdc("1000.000000"));
}

#[test]
fn incoming_transfers_are_revenue_and_never_obligations() {
    let mut txs = schedule("0xFEED", "500.000000", 30, 6);
    for t in txs.iter_mut() {
        t.direction = Direction::In;
    }
    assert!(detect(&txs, DetectionConfig::default()).is_empty());
}

#[test]
fn two_payments_are_never_a_pattern() {
    for n in 0..MIN_SAMPLES {
        let txs = schedule("0x1111", "20.000000", 30, n);
        assert!(detect(&txs, DetectionConfig::default()).is_empty(), "n={n}");
    }
}

#[test]
fn an_irregular_payee_is_refused_rather_than_reported_with_a_low_score() {
    let mut txs = schedule("0xBEEF", "20.000000", 30, 6);
    // Shove one payment far out of rhythm.
    txs[3].at = Timestamp::from_unix(T0 + 200 * 86_400);
    let found = detect(&txs, DetectionConfig::default());
    assert!(
        found.is_empty(),
        "no rhythm means no obligation, not a weak one"
    );
}

#[test]
fn mixed_case_addresses_are_one_payee_not_two() {
    let mut txs = schedule("0xAbCd", "20.000000", 30, 3);
    txs.extend(
        schedule("0xABCD", "20.000000", 30, 3)
            .into_iter()
            .enumerate()
            .map(|(i, mut t)| {
                t.at = Timestamp::from_unix(T0 + (3 + i as i64) * 30 * 86_400);
                t.tx_hash = format!("0xsecond{i}");
                t
            }),
    );
    let found = detect(&txs, DetectionConfig::default());
    assert_eq!(found.len(), 1, "0xAbCd and 0xABCD are the same payee");
    assert_eq!(found[0].sample_count, 6);
}

#[test]
fn the_same_payee_in_two_currencies_is_two_obligations() {
    let mut txs = schedule("0x9999", "20.000000", 30, 4);
    txs.extend((0..4).map(|i| Transfer {
        tx_hash: format!("0xeur{i}"),
        counterparty: Counterparty::new("0x9999"),
        amount: Money::parse("15.000000", Currency::Eurc).unwrap(),
        at: Timestamp::from_unix(T0 + i * 30 * 86_400),
        direction: Direction::Out,
    }));
    let found = detect(&txs, DetectionConfig::default());
    assert_eq!(
        found.len(),
        2,
        "separate currencies are separate obligations"
    );
    let currencies: Vec<Currency> = found.iter().map(|o| o.currency()).collect();
    assert!(currencies.contains(&Currency::Usdc));
    assert!(currencies.contains(&Currency::Eurc));
}

#[test]
fn simultaneous_payments_do_not_count_as_a_zero_interval() {
    // Two payments in the same second would otherwise contribute a gap of
    // zero and drag the median toward nothing.
    let mut txs = schedule("0x1111", "20.000000", 30, 6);
    txs[2].at = txs[1].at;
    let found = detect(&txs, DetectionConfig::default());
    if let Some(o) = found.first() {
        assert!(o.interval > Seconds::ZERO, "interval must be positive");
    }
}

#[test]
fn results_are_ordered_by_what_is_due_soonest() {
    let mut txs = schedule("0xLATE", "10.000000", 60, 4);
    txs.extend(schedule("0xSOON", "10.000000", 7, 4));
    let found = detect(&txs, DetectionConfig::default());
    assert_eq!(found.len(), 2);
    assert!(found[0].next_due <= found[1].next_due);
    assert_eq!(found[0].counterparty.as_str(), "0xsoon");
}

#[test]
fn the_cycle_key_is_stable_and_changes_with_the_cycle() {
    let txs = schedule("0x1111", "20.000000", 30, 6);
    let a = detect(&txs, DetectionConfig::default());
    let b = detect(&txs, DetectionConfig::default());
    assert_eq!(a[0].cycle_key(), b[0].cycle_key(), "same input, same key");
    assert_eq!(
        a[0].cycle_key(),
        format!("0x1111@{}", a[0].next_due.as_unix())
    );

    let longer = schedule("0x1111", "20.000000", 30, 7);
    let c = detect(&longer, DetectionConfig::default());
    assert_ne!(
        a[0].cycle_key(),
        c[0].cycle_key(),
        "a new cycle is a new key"
    );
}

#[test]
fn confidence_climbs_with_evidence_and_never_reaches_one() {
    let mut last = 0.0;
    for n in [3usize, 4, 6, 12, 24, 100] {
        let txs = schedule("0x1111", "20.000000", 30, n);
        let c = detect(&txs, DetectionConfig::default())[0].confidence.get();
        assert!(c > last, "n={n}: {c} should exceed {last}");
        assert!(c < 1.0, "n={n}: confidence must never reach certainty");
        last = c;
    }
}

#[test]
fn the_explanation_names_the_numbers_it_rests_on() {
    let txs = schedule("0x1111", "20.000000", 30, 6);
    let text = detect(&txs, DetectionConfig::default())[0].explain();
    assert!(text.contains("0x1111"), "{text}");
    assert!(text.contains("6 times"), "{text}");
    assert!(text.contains("30.0 days"), "{text}");
    assert!(text.contains("subscription"), "{text}");
    assert!(text.contains("0.85"), "{text}");
}

// ------------------------------------------------------------ aggregation

#[test]
fn upcoming_and_total_read_the_same_horizon() {
    let mut txs = schedule("0xSOON", "10.000000", 7, 4);
    txs.extend(schedule("0xLATE", "25.000000", 90, 4));
    let found = detect(&txs, DetectionConfig::default());

    let now = found.iter().map(|o| o.last_paid).max().unwrap();
    let within = upcoming(&found, now, Seconds::days(30).unwrap());
    assert_eq!(within.len(), 1, "only the weekly one falls inside 30 days");
    assert_eq!(
        total_due(within, Currency::Usdc).unwrap(),
        usdc("10.000000")
    );

    let all = upcoming(&found, now, Seconds::days(365).unwrap());
    assert_eq!(total_due(all, Currency::Usdc).unwrap(), usdc("35.000000"));
}

#[test]
fn totalling_across_currencies_refuses_rather_than_guesses() {
    let mut txs = schedule("0x9999", "20.000000", 30, 4);
    txs.extend((0..4).map(|i| Transfer {
        tx_hash: format!("0xeur{i}"),
        counterparty: Counterparty::new("0x8888"),
        amount: Money::parse("15.000000", Currency::Eurc).unwrap(),
        at: Timestamp::from_unix(T0 + i * 30 * 86_400),
        direction: Direction::Out,
    }));
    let found = detect(&txs, DetectionConfig::default());
    assert_eq!(found.len(), 2);
    assert!(
        total_due(&found, Currency::Usdc).is_none(),
        "USDC + EURC has no sum"
    );
}

// ------------------------------------------------------------- properties

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Arbitrary history, however malformed, must not panic and must not
    /// produce an out-of-range confidence.
    #[test]
    fn detection_is_total_and_confidence_stays_in_range(
        raw in prop::collection::vec(
            (any::<u8>(), 0i64..4_000_000_000, -1_000_000i128..1_000_000i128, any::<bool>()),
            0..40,
        )
    ) {
        let txs: Vec<Transfer> = raw
            .iter()
            .enumerate()
            .map(|(i, (who, at, minor, outgoing))| Transfer {
                tx_hash: format!("0x{i}"),
                counterparty: Counterparty::new(format!("0x{who:02x}")),
                amount: Money::minor(*minor, Currency::Usdc),
                at: Timestamp::from_unix(*at),
                direction: if *outgoing { Direction::Out } else { Direction::In },
            })
            .collect();

        for o in detect(&txs, DetectionConfig::default()) {
            prop_assert!((0.0..=1.0).contains(&o.confidence.get()));
            prop_assert!(o.sample_count >= MIN_SAMPLES);
            prop_assert!(o.interval > Seconds::ZERO);
            prop_assert!(o.interval_cv <= MAX_INTERVAL_CV);
            prop_assert_eq!(o.evidence.len(), o.sample_count);
        }
    }

    /// A schedule generated at a known cadence is recovered at that cadence.
    #[test]
    fn a_generated_cadence_is_recovered(
        days in 1i64..90,
        n in MIN_SAMPLES..30usize,
        // Above the dust line: one unit or less is never evidence (is_dust).
        minor in 1_000_001i128..1_000_000_000,
    ) {
        let txs: Vec<Transfer> = (0..n)
            .map(|i| Transfer {
                tx_hash: format!("0x{i}"),
                counterparty: Counterparty::new("0xabc"),
                amount: Money::minor(minor, Currency::Usdc),
                at: Timestamp::from_unix(T0 + i as i64 * days * 86_400),
                direction: Direction::Out,
            })
            .collect();

        let found = detect(&txs, DetectionConfig::default());
        prop_assert_eq!(found.len(), 1);
        prop_assert_eq!(found[0].interval, Seconds::days(days).unwrap());
        prop_assert_eq!(found[0].typical_amount.to_minor(), minor);
        prop_assert_eq!(found[0].kind, ObligationKind::Subscription);
        prop_assert_eq!(found[0].sample_count, n);
    }

    /// However regular, a schedule of dust is never an obligation. This is
    /// the shape an address poisoner writes into a history.
    #[test]
    fn dust_is_never_an_obligation(
        days in 1i64..90,
        n in MIN_SAMPLES..30usize,
        minor in 0i128..=1_000_000,
    ) {
        let txs: Vec<Transfer> = (0..n)
            .map(|i| Transfer {
                tx_hash: format!("0x{i}"),
                counterparty: Counterparty::new("0xbad"),
                amount: Money::minor(minor, Currency::Usdc),
                at: Timestamp::from_unix(T0 + i as i64 * days * 86_400),
                direction: Direction::Out,
            })
            .collect();
        prop_assert!(detect(&txs, DetectionConfig::default()).is_empty());
    }

    /// The typical amount is always one that was genuinely paid -- never an
    /// interpolated value nobody ever sent.
    #[test]
    fn the_typical_amount_was_actually_paid(
        amounts in prop::collection::vec(1_000_001i128..2_000_000, MIN_SAMPLES..20),
    ) {
        let txs: Vec<Transfer> = amounts
            .iter()
            .enumerate()
            .map(|(i, m)| Transfer {
                tx_hash: format!("0x{i}"),
                counterparty: Counterparty::new("0xabc"),
                amount: Money::minor(*m, Currency::Usdc),
                at: Timestamp::from_unix(T0 + i as i64 * 30 * 86_400),
                direction: Direction::Out,
            })
            .collect();

        for o in detect(&txs, DetectionConfig::default()) {
            prop_assert!(
                amounts.contains(&o.typical_amount.to_minor()),
                "{} was never paid", o.typical_amount
            );
        }
    }

    /// Order of arrival must not change the result. Logs come back in
    /// whatever order the node felt like.
    #[test]
    fn detection_does_not_depend_on_input_order(
        n in MIN_SAMPLES..15usize,
        shuffle_seed in any::<u64>(),
    ) {
        let txs = schedule("0x1111", "20.000000", 30, n);
        let mut shuffled = txs.clone();
        // Deterministic shuffle from the seed.
        let mut s = shuffle_seed | 1;
        for i in (1..shuffled.len()).rev() {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            shuffled.swap(i, (s >> 33) as usize % (i + 1));
        }
        prop_assert_eq!(
            detect(&txs, DetectionConfig::default()),
            detect(&shuffled, DetectionConfig::default())
        );
    }
}

#[test]
fn confidence_rejects_nonsense() {
    assert!(Confidence::new(f64::NAN).is_none());
    assert!(Confidence::new(f64::INFINITY).is_none());
    assert!(Confidence::new(-0.1).is_none());
    assert!(Confidence::new(1.1).is_none());
    assert_eq!(Confidence::new(0.85).unwrap().get(), 0.85);
    assert_eq!(Confidence::clamped(f64::NAN).get(), 0.0);
    assert_eq!(Confidence::clamped(5.0).get(), 1.0);
}

// ------------------------------------------------------- address poisoning

/// Address poisoning plants zero-value "outgoing" transfers to a look-alike so
/// the history reads as though the treasury paid it. docs/21 §7: 116 of 247
/// look-alikes were planted that way, and `detect` inferred obligations to
/// two of them. A zero-value or dust transfer is not evidence that anyone is
/// owed anything.
#[test]
fn zero_value_and_dust_transfers_are_not_evidence_of_a_payee() {
    let mut txs = schedule("0x2222", "0.000000", 30, 6);
    txs.extend(schedule("0x3333", "0.500000", 30, 6));
    txs.extend(schedule("0x5555", "1.000000", 30, 6));
    txs.extend(schedule("0x1111", "20.000000", 30, 6));
    let found = detect(&txs, DetectionConfig::default());
    let who: Vec<&str> = found.iter().map(|o| o.counterparty.as_str()).collect();
    assert_eq!(who, vec!["0x1111"], "only the real payee is an obligation");
}

#[test]
fn a_payment_just_above_the_dust_line_still_counts() {
    let txs = schedule("0x4444", "1.000001", 30, 6);
    assert_eq!(detect(&txs, DetectionConfig::default()).len(), 1);
}

#[test]
fn dust_mixed_into_a_real_schedule_does_not_change_it() {
    let mut txs = schedule("0x1111", "20.000000", 30, 6);
    // Same counterparty, a poisoner's zero-value entries in between.
    txs.extend((0..6).map(|i| Transfer {
        tx_hash: format!("0xdust{i:04}"),
        counterparty: Counterparty::new("0x1111"),
        amount: usdc("0.000000"),
        at: Timestamp::from_unix(T0 + 15 * 86_400 + (i as i64) * 30 * 86_400),
        direction: Direction::Out,
    }));
    let found = detect(&txs, DetectionConfig::default());
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0].sample_count, 6,
        "the zero-value entries are not samples"
    );
    assert_eq!(found[0].typical_amount, usdc("20.000000"));
}

/// A demonstration scaled down to thousandths of a USDC lowers the dust line
/// with it. The payments are recovered; the zero-value entries still are not.
#[test]
fn a_scaled_down_dust_line_keeps_small_payments_and_still_drops_zero() {
    let config = DetectionConfig {
        dust_line: Some(usdc("0.001000")),
        ..DetectionConfig::default()
    };
    let mut txs = schedule("0x00a1", "0.002000", 30, 5);
    txs.extend(schedule("0x00a9", "0.000000", 30, 5));
    let found = detect(&txs, config);
    let who: Vec<&str> = found.iter().map(|o| o.counterparty.as_str()).collect();
    assert_eq!(who, vec!["0x00a1"]);
    assert!(
        detect(&txs, DetectionConfig::default()).is_empty(),
        "default line is one USDC"
    );
}
