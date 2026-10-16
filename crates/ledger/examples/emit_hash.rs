//! Prints the action hash for the canonical demo action, for use by the
//! on-chain end-to-end proof.  cargo run --quiet --example emit_hash
use ledger::*;
use std::collections::{BTreeMap, BTreeSet};

const NOW: u64 = 1_764_000_000;
fn usdc(s: &str) -> Money {
    Money::parse(s, Currency::Usdc).unwrap()
}
fn treasury() -> AccountId {
    AccountId::new("asset:usdc")
}

fn main() {
    let authority = Authority {
        per_action_cap: usdc("100.000000"),
        daily_cap: usdc("500.000000"),
        spent_today: usdc("0.000000"),
    };
    let action = Action {
        id: "act_0001".into(),
        actor: Actor {
            id: "claude-reasoner".into(),
            kind: ActorKind::Model,
        },
        principal: Principal {
            id: "org_tameion".into(),
            treasury: treasury(),
        },
        intent: Intent {
            verb: Verb::Settle,
            rationale: "0x1111 is due, 20 USDC, paid every 30 days for six cycles".into(),
        },
        target: Target {
            id: "0x1111".into(),
            label: Some("SaaS vendor".into()),
        },
        amount: usdc("20.000000"),
        evidence: (0..6)
            .map(|i| Evidence {
                kind: EvidenceKind::OnchainTransfer,
                reference: format!("0x6a858e1a{i:02}"),
                provenance: Provenance::Onchain,
            })
            .collect(),
        preconditions: vec![
            Precondition::CounterpartyKnown,
            Precondition::EvidenceAtLeast(3),
        ],
        authority: authority.clone(),
        expected_effects: vec![ExpectedEffect {
            account: treasury(),
            delta: Delta::Decrease(usdc("20.000000")),
        }],
        idempotency: Idempotency {
            key: "0x1111@1984916082".into(),
            scope: "org_tameion".into(),
        },
        validity: Validity {
            not_before: NOW - 60,
            expires_at: NOW + 3600,
        },
    };

    let mut known = BTreeMap::new();
    known.insert("0x1111".to_string(), "fingerprint-A".to_string());
    let world = World {
        now: NOW,
        treasury: treasury(),
        balance: usdc("1000.000000"),
        authority,
        settled: BTreeSet::new(),
        known_payees: known.clone(),
        proposed_payee: known,
        required_preconditions: vec![
            Precondition::NotAlreadySettled,
            Precondition::EvidenceAtLeast(1),
        ],
        attested_payees: BTreeMap::new(),
        inferred_confidence: BTreeMap::new(),
        purchase_orders: BTreeMap::new(),
        receipts: BTreeMap::new(),
        onchain_attested: None,
        cycles_behind: BTreeMap::new(),
        observed_amounts: BTreeMap::new(),
        issuer_blocked: BTreeSet::new(),
    };

    let verdict = verify(&action, &world);
    // Machine-readable: DECISION, HASH, AMOUNT_MINOR, TARGET
    println!("DECISION={}", verdict.decision);
    println!("HASH={}", verdict.action_hash);
    println!("AMOUNT_MINOR={}", action.amount.to_minor());
    println!("TARGET={}", action.target.id);
}
