//! What an outside agent is told (docs/110 U3; docs/60 P-180…P-182).
//!
//! An agent reads the account through MCP/API and relays it to a person, so
//! the words have to be the whole truth: one list of everything that waits
//! for the owner (the same items the account page counts), who signs a
//! prepared call and when it takes effect, and the agent's own requests kept
//! so the owner's only act is one tap on the page.

use crate::level1;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

/// Every item that waits for the owner, in one list and in the order the
/// account page shows them: sellers to approve, invoices, bills to approve.
/// These are exactly the three lists the page counts in "Needs you"
/// (`needsYou`, `sellersToApprove`, `invoices`), so an agent that reads this
/// and the page that shows it cannot disagree about whether anything waits.
pub fn waiting(needs_you: &[Value], sellers: &[Value], invoices: &[Value]) -> Vec<Value> {
    let tap = "the owner's tap on the account page (Needs you)";
    let mut out = Vec::new();
    for s in sellers {
        out.push(json!({
            "kind": "seller",
            "what": format!("approve {} so the agent may pay it", s["host"].as_str().filter(|h| !h.is_empty()).unwrap_or("a seller")),
            "counterparty": s["seller"], "amount": s["amount"], "why": s["reason"], "decidedBy": tap,
        }));
    }
    for i in invoices {
        out.push(json!({
            "kind": "invoice",
            "what": "accept or decline an invoice a vendor sent",
            "counterparty": i["payee"], "amount": i["amountText"].as_str().map_or_else(|| i["amount"].clone(), |a| json!(a)),
            "dueAt": i["dueAt"], "decidedBy": tap,
        }));
    }
    for n in needs_you {
        out.push(json!({
            "kind": "bill",
            "what": "approve a payment the agent prepared",
            "counterparty": n["counterparty"], "amount": n["amount"], "why": n["why"], "decidedBy": tap,
        }));
    }
    out
}

/// Why a bill exists, in words, from a pass's `obligationsList` entry: the
/// reason every payment row, CSV line and Needs-you row carries (docs/60
/// P-177: "why" was missing from the book).
pub fn bill_reason(o: &Value) -> String {
    let amount = o["amount"].as_str().unwrap_or("?");
    let days = o["intervalSecs"].as_u64().map_or(0, |s| s / 86_400);
    let every = if days > 0 {
        format!("{amount} every {days} days")
    } else {
        amount.to_string()
    };
    match o["provenance"].as_str().unwrap_or("") {
        "imported" => format!("a bill the owner typed: {every}"),
        "x402 receipt" => format!("a paid API the agent buys from on a cadence: {every}"),
        p if p.starts_with("invoice") => format!("an invoice the payee signed: {every}"),
        _ => format!(
            "a bill found in the payment history: paid {} times, {every}",
            o["samples"].as_u64().unwrap_or(0)
        ),
    }
}

/// The reason for a payment to `counterparty`, from the newest pass report
/// that knows the bill; `None` when no pass has seen it.
pub fn reason_for(obligations: &[Value], counterparty: &str) -> Option<String> {
    obligations
        .iter()
        .find(|o| {
            o["counterparty"]
                .as_str()
                .is_some_and(|c| c.eq_ignore_ascii_case(counterparty))
        })
        .map(bill_reason)
}

/// Whether an owner call relayed to the account only shrinks what the agent
/// may do — a cancel of the pending change, a pause, revoking payees — and
/// which (docs/60 P-171). Such a call must never lose a race to the automatic
/// apply of the very raise it answers. Calldata as hex.
pub fn shrinks_trust(data: &str) -> Option<&'static str> {
    let hex = data.trim().trim_start_matches("0x").to_lowercase();
    let arg = |i: usize| hex.get(8 + 64 * i..8 + 64 * (i + 1));
    let zero = |w: Option<&str>| w.is_some_and(|w| w.bytes().all(|b| b == b'0'));
    match hex.get(..8)? {
        "dedad97d" => Some("cancel"),
        // setMode(uint8) with 0 (Watch): a pause.
        "21175b4a" if zero(arg(0)) => Some("pause"),
        // setPayees(address[],bool) / setPayeesIf(address[],bool,bytes32,uint64) with false.
        "48a56503" | "3948f4ff" if zero(arg(1)) => Some("revoke"),
        _ => None,
    }
}

/// Who signs every prepared call: the owner's passkey, never the caller.
pub const SIGNER: &str =
    "only the owner's passkey can sign this, on the account page; an agent key cannot";

/// When a limits change takes effect, by the contract's own rule
/// (`BudgetPolicy.setLimits`): at once when it only shrinks what the agent may
/// do (both caps lower or equal, the floor higher or equal), otherwise after
/// the delay, and the owner can cancel it until then. Minor units.
pub fn limits_apply(current: [i128; 3], asked: [i128; 3], delay_secs: u64) -> String {
    let [per, day, floor] = current;
    let [new_per, new_day, new_floor] = asked;
    if current == asked {
        "nothing changes: these are the limits now".into()
    } else if new_per <= per && new_day <= day && new_floor >= floor {
        "at once when signed: it only lowers what the agent may spend".into()
    } else {
        format!(
            "{} after it is signed, and the owner can cancel it until then: it raises what the agent may spend",
            delay_words(delay_secs)
        )
    }
}

/// When a change of autonomy level takes effect (`BudgetPolicy.setMode`):
/// lowering at once, raising after the delay, cancellable.
pub fn mode_apply(current: u8, asked: u8, delay_secs: u64) -> String {
    if asked == current {
        "nothing changes: the account is on that level now".into()
    } else if asked < current {
        "at once when signed: it lowers what the agent may do by itself".into()
    } else {
        format!(
            "{} after it is signed, and the owner can cancel it until then: it raises what the agent may do by itself",
            delay_words(delay_secs)
        )
    }
}

/// When a payee approval or revocation takes effect.
pub fn payees_apply(allow: bool, delay_secs: u64) -> String {
    if allow {
        format!(
            "payable {} after it is signed (the contract's cooling-off, so the owner can still revoke)",
            delay_words(delay_secs)
        )
    } else {
        "at once when signed: the agent can no longer pay them".into()
    }
}

fn delay_words(secs: u64) -> String {
    match secs {
        0 => "immediately".into(),
        s if s % 86_400 == 0 => format!("{} day(s)", s / 86_400),
        s if s % 3_600 == 0 => format!("{} hour(s)", s / 3_600),
        s if s % 60 == 0 => format!("{} minute(s)", s / 60),
        s => format!("{s} seconds"),
    }
}

/// The autonomy level a status label names (`Watch`/`Paused` 0, `Propose` 1,
/// `Autopilot` 2).
pub fn mode_number(label: &str) -> u8 {
    match label {
        "Autopilot" => 2,
        "Propose" => 1,
        _ => 0,
    }
}

/// How long an agent's request is shown before it lapses unanswered.
const ASK_LIFE_SECS: u64 = 7 * 86_400;

fn asks_path(app_dir: &Path, policy: &str) -> PathBuf {
    level1::tenants_dir(app_dir).join(format!("{}.asks.json", policy.trim().to_lowercase()))
}

/// Keep what an agent key asked the owner for (`kind` = `limits`, `mode` or
/// `payees`), replacing that kind's earlier request: the owner sees one row per
/// kind, with one tap, and decides.
pub fn record_ask(app_dir: &Path, policy: &str, kind: &str, ask: Value) -> Result<(), String> {
    let p = asks_path(app_dir, policy);
    let mut all: Value = fs::read_to_string(&p)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    all[kind] = ask;
    if let Some(d) = p.parent() {
        fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    fs::write(&p, all.to_string()).map_err(|e| e.to_string())
}

/// The agent's requests still open at `now` (a request older than a week has
/// lapsed); an object keyed by kind.
pub fn asks(app_dir: &Path, policy: &str, now: u64) -> Value {
    let all: Value = fs::read_to_string(asks_path(app_dir, policy))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}));
    let mut open = json!({});
    for (k, v) in all.as_object().into_iter().flatten() {
        if v["at"]
            .as_u64()
            .is_some_and(|at| now.saturating_sub(at) < ASK_LIFE_SECS)
        {
            open[k] = v.clone();
        }
    }
    open
}

/// A pending change in words, naming only what is pending (`kinds` bit 0 =
/// limits, bit 1 = mode): a mode-only change never reads as caps going to 0.
pub fn pending_words(kinds: u64, mode: &str, caps: [&str; 3], eta: u64) -> String {
    let mut what = Vec::new();
    if kinds & 1 != 0 {
        what.push(format!(
            "limits per payment {}, per day {}, floor {}",
            caps[0], caps[1], caps[2]
        ));
    }
    if kinds & 2 != 0 {
        what.push(format!("autonomy {mode}"));
    }
    format!(
        "{} from unix {eta}; the owner can cancel it until then",
        what.join(" and ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiting_holds_the_three_lists_the_page_counts() {
        let sellers = [
            json!({ "seller": "0xa", "host": "api.example", "amount": "0.002", "reason": "weather" }),
        ];
        let invoices = [json!({ "payee": "0xb", "amountText": "0.15 USDC", "dueAt": 1 })];
        let bills = [json!({ "counterparty": "0xc", "amount": "1 USDC", "why": "rent" })];
        let w = waiting(&bills, &sellers, &invoices);
        assert_eq!(w.len(), 3, "one row per item the page counts");
        assert_eq!(
            w.iter()
                .map(|x| x["kind"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["seller", "invoice", "bill"]
        );
        assert_eq!(w[0]["why"], "weather");
        assert_eq!(w[1]["amount"], "0.15 USDC");
        assert!(waiting(&[], &[], &[]).is_empty());
    }

    #[test]
    fn a_cut_applies_at_once_and_a_raise_waits() {
        let cur = [3000, 8000, 0];
        assert!(limits_apply(cur, [2000, 8000, 0], 60).starts_with("at once"));
        assert!(
            limits_apply(cur, [3000, 8000, 100], 60).starts_with("at once"),
            "a higher floor shrinks"
        );
        let raise = limits_apply(cur, [4000, 8000, 0], 60);
        assert!(
            raise.starts_with("1 minute(s) after") && raise.contains("cancel"),
            "{raise}"
        );
        assert!(
            limits_apply(cur, [2000, 9000, 0], 60).contains("cancel"),
            "any raise waits"
        );
        assert!(mode_apply(1, 2, 3600).starts_with("1 hour(s) after"));
        assert!(mode_apply(2, 2, 3600).starts_with("nothing changes"));
        assert!(limits_apply(cur, cur, 60).starts_with("nothing changes"));
        assert!(mode_apply(2, 0, 3600).starts_with("at once"));
        assert!(payees_apply(true, 60).contains("1 minute(s)"));
        assert!(payees_apply(false, 60).starts_with("at once"));
    }

    #[test]
    fn a_cancel_a_pause_and_a_revoke_are_trust_shrinking() {
        let word = |v: u64| format!("{v:064x}");
        assert_eq!(shrinks_trust("0xdedad97d"), Some("cancel"));
        assert_eq!(
            shrinks_trust(&format!("0x21175b4a{}", word(0))),
            Some("pause")
        );
        assert_eq!(
            shrinks_trust(&format!("0x21175b4a{}", word(2))),
            None,
            "Autopilot is a raise"
        );
        assert_eq!(
            shrinks_trust(&format!("0x48a56503{}{}", word(64), word(0))),
            Some("revoke")
        );
        assert_eq!(
            shrinks_trust(&format!("0x48a56503{}{}", word(64), word(1))),
            None,
            "approving payees widens trust"
        );
        assert_eq!(shrinks_trust("0x"), None);
    }

    #[test]
    fn every_payment_says_why_the_bill_exists() {
        let typed = json!({ "amount": "0.050000 USDC", "counterparty": "0xAbC", "intervalSecs": 2_592_000, "provenance": "imported", "samples": 0 });
        assert_eq!(
            bill_reason(&typed),
            "a bill the owner typed: 0.050000 USDC every 30 days"
        );
        let found = json!({ "amount": "1 USDC", "counterparty": "0xd", "intervalSecs": 604_800, "provenance": "history", "samples": 6 });
        assert_eq!(
            bill_reason(&found),
            "a bill found in the payment history: paid 6 times, 1 USDC every 7 days"
        );
        assert_eq!(
            reason_for(&[typed, found], "0xabc").as_deref(),
            Some("a bill the owner typed: 0.050000 USDC every 30 days")
        );
        assert_eq!(reason_for(&[], "0xabc"), None);
    }

    #[test]
    fn a_mode_only_change_never_reads_as_caps_of_zero() {
        let w = pending_words(2, "Autopilot", ["0.000000 USDC"; 3], 1_792_154_473);
        assert_eq!(
            w,
            "autonomy Autopilot from unix 1792154473; the owner can cancel it until then"
        );
        assert!(!w.contains("0.000000"));
        let both = pending_words(3, "Autopilot", ["0.004", "0.008", "0"], 5);
        assert!(both.contains("per payment 0.004") && both.contains("autonomy Autopilot"));
    }

    #[test]
    fn an_agents_request_is_kept_per_kind_and_lapses() {
        let dir = std::env::temp_dir().join(format!("agent-asks-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let p = "0xb32753851113869778fec66c51c3a13405edcfab";
        record_ask(
            &dir,
            p,
            "limits",
            json!({ "perAction": "0.004", "at": 100 }),
        )
        .unwrap();
        record_ask(&dir, p, "mode", json!({ "mode": 2, "at": 100 })).unwrap();
        record_ask(
            &dir,
            p,
            "limits",
            json!({ "perAction": "0.005", "at": 200 }),
        )
        .unwrap();
        let open = asks(&dir, p, 300);
        assert_eq!(
            open["limits"]["perAction"], "0.005",
            "the newest request of a kind replaces the older"
        );
        assert_eq!(open["mode"]["mode"], 2);
        assert_eq!(
            asks(&dir, p, 100 + ASK_LIFE_SECS),
            json!({ "limits": { "perAction": "0.005", "at": 200 } })
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
