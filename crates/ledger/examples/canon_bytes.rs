use ledger::*;
fn main() {
    let m = |s: &str| Money::parse(s, Currency::Usdc).unwrap();
    let a = Action {
        id: "act_0001".into(),
        actor: Actor {
            id: "claude-reasoner".into(),
            kind: ActorKind::Model,
        },
        principal: Principal {
            id: "org".into(),
            treasury: AccountId::new("asset:usdc"),
        },
        intent: Intent {
            verb: Verb::Settle,
            rationale: "due".into(),
        },
        target: Target {
            id: "0x1111".into(),
            label: None,
        },
        amount: m("20.000000"),
        evidence: vec![Evidence {
            kind: EvidenceKind::OnchainTransfer,
            reference: "0xaa".into(),
            provenance: Provenance::Onchain,
        }],
        preconditions: vec![Precondition::NotAlreadySettled],
        authority: Authority {
            per_action_cap: m("100.000000"),
            daily_cap: m("500.000000"),
            spent_today: m("0.000000"),
        },
        expected_effects: vec![ExpectedEffect {
            account: AccountId::new("asset:usdc"),
            delta: Delta::Decrease(m("20.000000")),
        }],
        idempotency: Idempotency {
            key: "k".into(),
            scope: "org".into(),
        },
        validity: Validity {
            not_before: 0,
            expires_at: u64::MAX,
        },
    };
    let bytes = canonicalize(&a);
    print!("0x");
    for b in &bytes {
        print!("{b:02x}");
    }
    println!();
    eprintln!("{}", action_hash(&a));
}
