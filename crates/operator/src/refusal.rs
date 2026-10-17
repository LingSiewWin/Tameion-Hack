//! One reply shape for every agent (docs/115 §2, docs/114 §1 rows 1, 2, 3, 5, 7).
//!
//! Every answer an agent gets carries `ok` and the contract's date. A refusal is
//! one object an agent can act on without reading prose: a `code` from one
//! catalogue ([`CODES`], served at `GET /api/level1/codes` and listed in
//! `llms.txt`), who refused (`by`), whether anything was done (`executed`, read
//! from the chain where a payment may have gone out), who must act
//! (`audience`), what it means and what to do next, beside the sentence a
//! person reads (`words`).

use serde_json::{Value, json};

/// The date-stamped version of this contract; a client warns once when it
/// changes.
pub const CONTRACT_VERSION: &str = "2026-10-17.1";

/// One refusal code of the catalogue.
pub struct Code {
    pub name: &'static str,
    /// Who refused: one of [`BY`].
    pub by: &'static str,
    /// `user` when a human must act (approve a payee, raise a limit, switch
    /// mode, sign); otherwise `agent`.
    pub audience: &'static str,
    pub meaning: &'static str,
    /// What to do, when the refusing site says nothing more specific.
    pub next_step: &'static str,
}

/// Who can refuse.
pub const BY: &[&str] = &[
    "auth",
    "input",
    "mandate",
    "budget",
    "payee",
    "idempotency",
    "chain",
    "upstream",
    "service",
];

const fn c(
    name: &'static str,
    by: &'static str,
    audience: &'static str,
    meaning: &'static str,
    next_step: &'static str,
) -> Code {
    Code {
        name,
        by,
        audience,
        meaning,
        next_step,
    }
}

/// Every refusal an agent can get. A refusal without a code here is a bug.
pub const CODES: &[Code] = &[
    c(
        "token_missing",
        "auth",
        "agent",
        "This account requires an agent key and the request carried none.",
        "Send Authorization: Bearer <agent key> (TAMEION_TOKEN for tameion-mcp); the owner mints one on the account page.",
    ),
    c(
        "token_invalid",
        "auth",
        "agent",
        "The agent key is not one this account issued: mistyped, revoked, or for another account.",
        "Check the key you send; if the owner revoked it, ask them for a new one.",
    ),
    c(
        "token_expired",
        "auth",
        "user",
        "The agent key has expired.",
        "Ask the owner to mint a new agent key on the account page.",
    ),
    c(
        "token_scope",
        "auth",
        "user",
        "The agent key does not carry the scope this call needs (named in `scope`).",
        "Ask the owner to mint a key with that scope on the account page.",
    ),
    c(
        "token_stopped",
        "auth",
        "user",
        "The owner stopped this agent key.",
        "Stop calling with this key: a stopped key stays stopped; ask the owner for a new one.",
    ),
    c(
        "needs_owner",
        "auth",
        "user",
        "Only the owner can do this: their passkey, or the device that opened the account.",
        "Ask the owner to do it on the account page.",
    ),
    c(
        "mode_watch",
        "mandate",
        "user",
        "The account is on Watch (paused): the agent pays nothing by itself.",
        "Ask the owner to raise the autonomy level (prepare_mode); a raise waits the account's delay.",
    ),
    c(
        "mode_propose",
        "mandate",
        "user",
        "The account is on Propose: the agent asks before every payment and pays nothing by itself.",
        "Ask the owner to turn on Autopilot (prepare_mode with mode 2); the switch waits the account's delay and the owner can cancel it.",
    ),
    c(
        "agent_lapsed",
        "mandate",
        "user",
        "The agent's authority to pay has lapsed.",
        "Ask the owner to renew it (prepare_renew).",
    ),
    c(
        "account_keyless",
        "mandate",
        "user",
        "This account has no agent key, so its agent cannot pay or buy by itself.",
        "The owner approves each payment on the account page.",
    ),
    c(
        "account_not_deployed",
        "chain",
        "user",
        "The account is not on chain yet.",
        "The owner funds the account or taps to open it; the first check puts it on chain.",
    ),
    c(
        "price_out_of_band",
        "mandate",
        "user",
        "The price is more than a quarter above what this seller usually charges, so the owner decides.",
        "The ask waits in the owner's Needs you; one tap lets one purchase through at that price.",
    ),
    c(
        "over_per_payment",
        "budget",
        "user",
        "The amount is above the account's per-payment limit.",
        "Pay less, or ask the owner to raise the limit (prepare_limits); a raise waits the account's delay.",
    ),
    c(
        "over_24h",
        "budget",
        "agent",
        "The amount is above what is left of the trailing 24-hour limit.",
        "Wait until the 24-hour window frees room (caps.leftNow in treasury_inbox), or ask the owner to raise the daily limit.",
    ),
    c(
        "below_floor",
        "budget",
        "user",
        "Paying would take the balance under the floor the owner set.",
        "Ask the owner to fund the account or lower the floor.",
    ),
    c(
        "insufficient_funds",
        "budget",
        "user",
        "The account does not hold enough USDC for this payment.",
        "Ask the owner to fund the account.",
    ),
    c(
        "over_max_price",
        "budget",
        "agent",
        "The seller's price is above the most the caller allowed for this call (maxUsdc).",
        "Raise maxUsdc if the price is worth it, or skip the call.",
    ),
    c(
        "payee_not_approved",
        "payee",
        "user",
        "The payee is not on the account's payee list.",
        "Ask the owner to approve the payee; the ask is in Needs you.",
    ),
    c(
        "payee_cooling_off",
        "payee",
        "agent",
        "The payee was approved too recently; it can be paid once its waiting period ends.",
        "Try again after the payee's payableFrom time (payees in treasury_inbox).",
    ),
    c(
        "payee_frozen",
        "payee",
        "user",
        "The stablecoin issuer froze this payee: USDC refuses transfers to it.",
        "Ask the owner for another payee; this one cannot be paid.",
    ),
    c(
        "idempotency_conflict",
        "idempotency",
        "agent",
        "This Idempotency-Key was already used with a different body.",
        "Use a new key for a new request; resend the first body to get the first answer.",
    ),
    c(
        "idempotency_pending",
        "idempotency",
        "agent",
        "A request with this Idempotency-Key is still running.",
        "Resend the same request with the same key after retryAfterSecs.",
    ),
    c(
        "seller_pending",
        "idempotency",
        "agent",
        "An earlier purchase from this seller can still be collected; a second is never signed until it settles or expires.",
        "Read purchases, then buy again after the deadline in words.",
    ),
    c(
        "already_paid",
        "idempotency",
        "agent",
        "This payment was already made.",
        "Do not pay it again; read purchases or treasury_inbox for the receipt.",
    ),
    c(
        "invalid_body",
        "input",
        "agent",
        "The request is missing a field or carries one this route cannot use.",
        "Fix the request as words says and send it again.",
    ),
    c(
        "not_found",
        "input",
        "agent",
        "No such account, job or record on this service.",
        "Check the address or id; an account lives on the service that opened it.",
    ),
    c(
        "body_too_large",
        "input",
        "agent",
        "The request body is larger than this route takes.",
        "Send a smaller body.",
    ),
    c(
        "state_conflict",
        "input",
        "agent",
        "The request does not fit the account as it is now.",
        "Read the account (treasury_inbox), adjust the request, and send it again.",
    ),
    c(
        "rate_limited",
        "service",
        "agent",
        "Too many requests of this kind just now.",
        "Resend once after retryAfterSecs.",
    ),
    c(
        "account_busy",
        "service",
        "agent",
        "Another payment for this account was being signed; nothing was signed or paid.",
        "Try again in a few seconds.",
    ),
    c(
        "service_unavailable",
        "service",
        "agent",
        "This service could not do it just now; nothing was done.",
        "Try again in a few seconds.",
    ),
    c(
        "service_paused",
        "service",
        "agent",
        "The operator paused this part of the service (the demo's relayer is under its floor); nothing was done.",
        "Try again later, or tell the operator.",
    ),
    c(
        "service_error",
        "service",
        "agent",
        "This service failed to do it.",
        "Read the account before trying again; tell the operator if it repeats.",
    ),
    c(
        "chain_unavailable",
        "chain",
        "agent",
        "The chain did not answer in time.",
        "Read the account before trying again, so nothing is asked twice.",
    ),
    c(
        "chain_refused",
        "chain",
        "agent",
        "The account's contract refused the payment for a reason not in this catalogue (in words).",
        "Read treasury_inbox; tell the owner what words says.",
    ),
    c(
        "purchase_expired",
        "chain",
        "agent",
        "The purchase expired before the seller collected it; nothing was paid.",
        "Buy again if it is still needed.",
    ),
    c(
        "upstream_unavailable",
        "upstream",
        "agent",
        "A service this one depends on (a seller, Circle, a node) did not answer as expected.",
        "Read the account before trying again; if executed is not \"no\", never resend a payment.",
    ),
    c(
        "x402_unsupported",
        "upstream",
        "agent",
        "The seller does not take a payment this account can make (USDC, the exact scheme, this chain).",
        "Use another seller.",
    ),
];

/// Whether the refused action was carried out: read from the chain where a
/// payment may have gone out; `Unknown` when that read failed or the seller
/// may still collect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Executed {
    Yes,
    No,
    Unknown,
}

impl Executed {
    pub fn as_str(self) -> &'static str {
        match self {
            Executed::Yes => "yes",
            Executed::No => "no",
            Executed::Unknown => "unknown",
        }
    }
}

/// The catalogue entry of `name`.
pub fn code(name: &str) -> Option<&'static Code> {
    CODES.iter().find(|c| c.name == name)
}

/// A refusal in the envelope. `meaning` and `next_step` left empty take the
/// catalogue's. An unknown code panics in tests and is logged otherwise (and
/// answered as `service_error`'s `by` and `audience`).
pub fn refuse(
    code_name: &str,
    executed: Executed,
    meaning: impl Into<String>,
    next_step: impl Into<String>,
    words: impl Into<String>,
    retry_after: Option<u64>,
) -> Value {
    let entry = code(code_name).unwrap_or_else(|| {
        if cfg!(test) {
            panic!("refusal code {code_name} is not in refusal::CODES");
        }
        eprintln!("refusal: code {code_name} is not in the catalogue");
        code("service_error").expect("service_error is in the catalogue")
    });
    let or = |s: String, d: &str| {
        if s.trim().is_empty() {
            d.to_string()
        } else {
            s
        }
    };
    json!({
        "ok": false,
        "contractVersion": CONTRACT_VERSION,
        "error": {
            "code": code_name,
            "by": entry.by,
            "executed": executed.as_str(),
            "audience": entry.audience,
            "meaning": or(meaning.into(), entry.meaning),
            "nextStep": or(next_step.into(), entry.next_step),
            "retryAfterSecs": retry_after.unwrap_or(0),
            "words": words.into(),
        }
    })
}

/// An answer in the envelope: the route's fields, unchanged, plus `ok` and
/// `contractVersion`. Anything but an object is returned as it is.
pub fn ok(mut v: Value) -> Value {
    if let Some(o) = v.as_object_mut() {
        o.insert("ok".into(), json!(true));
        o.insert("contractVersion".into(), json!(CONTRACT_VERSION));
    }
    v
}

/// `fields` of `extra` (e.g. a purchase line, the scope) beside a refusal.
pub fn with(mut refusal: Value, extra: &[(&str, Value)]) -> Value {
    for (k, v) in extra {
        refusal[*k] = v.clone();
    }
    refusal
}

/// The code a refusal gets from its HTTP status when its site gave none: the
/// route's own sentence stays in `words`.
pub fn from_status(status: u16) -> &'static str {
    match status {
        401 => "token_missing",
        403 => "needs_owner",
        404 => "not_found",
        409 => "state_conflict",
        413 => "body_too_large",
        429 => "rate_limited",
        502 | 504 => "upstream_unavailable",
        503 => "chain_unavailable",
        500..=599 => "service_error",
        _ => "invalid_body",
    }
}

/// The routes an agent calls: every route the MCP tools call, every route an
/// agent key opens (`level1::scope_for`), and the catalogue.
pub fn agent_route(method: &str, path: &str) -> bool {
    crate::level1::scope_for(method, path).is_some()
        || matches!(
            (method, path),
            ("GET", "/api/level1/events")
                | ("GET", "/api/level1/codes")
                | ("GET", "/api/level1/status")
                | ("GET", "/api/level1/records")
                | ("GET", "/api/level1/record")
                | ("POST", "/api/level1/identity")
                | ("POST", "/api/level1/wallets")
                | ("POST", "/api/level1/invoice")
                | ("POST", "/api/watch")
        )
        || (method == "GET" && path.starts_with("/api/watch/"))
}

/// Routes whose work can move money or send a transaction: a failure there
/// after the work started is `executed: unknown`, never `no`.
pub fn moves_money(path: &str) -> bool {
    matches!(
        path,
        "/api/level1/buy"
            | "/api/level1/pay"
            | "/api/level1/fund"
            | "/api/level1/run"
            | "/api/level1/identity"
    )
}

/// The envelope for a reply its route built without one: an answer gains
/// `ok` and `contractVersion`; a refusal whose `error` is the route's sentence
/// becomes the catalogue's object (code from the HTTP status), every other
/// field kept. `None` when the reply is already in the envelope or is not an
/// object.
pub fn envelope(status: u16, v: &Value, money: bool) -> Option<Value> {
    let o = v.as_object()?;
    if o.contains_key("ok") {
        return None;
    }
    if (200..300).contains(&status) {
        return Some(ok(v.clone()));
    }
    let words = match &v["error"] {
        Value::String(s) => s.clone(),
        Value::Null => v["words"].as_str().unwrap_or("").to_string(),
        other => other.to_string(),
    };
    let code_name = from_status(status);
    let executed = if status >= 500 && money {
        Executed::Unknown
    } else {
        Executed::No
    };
    let retry = (code_name == "rate_limited").then_some(60);
    let mut out = refuse(code_name, executed, "", "", words, retry);
    for (k, x) in o.iter().filter(|(k, _)| k.as_str() != "error") {
        out[k.as_str()] = x.clone();
    }
    Some(out)
}

/// A refusal as a page shows it: `error` back to the sentence (`words`), so
/// the console and the classic pages read what they always read. `None` when
/// nothing changes.
pub fn for_page(v: &Value) -> Option<Value> {
    let words = v["error"]["words"].as_str()?;
    let mut out = v.clone();
    out["error"] = json!(words);
    Some(out)
}

/// Whether a request comes from a browser page, by its head: browsers send
/// `Sec-Fetch-Site` on every request and scripts cannot set it. Not
/// `Sec-Fetch-Mode`: Node's `fetch` sends that one too, so the SDK was read as a
/// page (docs/60 P-274).
pub fn from_page(request: &str) -> bool {
    let head = request.split("\r\n\r\n").next().unwrap_or("");
    head.to_ascii_lowercase().contains("\r\nsec-fetch-site:")
}

/// The code of an agent-key refusal (`level1::token_allows`'s words).
pub fn token_code(words: &str) -> &'static str {
    if words.starts_with("buying a service needs") || words.contains("does not carry the scope") {
        "token_scope"
    } else if words.contains("requires an agent key") {
        "token_missing"
    } else if words.contains("expired") {
        "token_expired"
    } else if words.contains("stopped this agent key") {
        "token_stopped"
    } else {
        "token_invalid"
    }
}

/// What changed in this contract and the service, newest first
/// (`apps/classic/updates.json`): `[{ "date", "note" }]`.
pub fn recent_updates() -> Value {
    serde_json::from_str(include_str!("../../../apps/classic/updates.json"))
        .unwrap_or_else(|_| json!([]))
}

/// The catalogue as `GET /api/level1/codes` serves it.
pub fn catalogue() -> Value {
    ok(json!({
        "codes": CODES.iter().map(|c| json!({
            "code": c.name, "by": c.by, "audience": c.audience, "meaning": c.meaning, "nextStep": c.next_step,
        })).collect::<Vec<_>>(),
        "by": BY,
        "executed": ["yes", "no", "unknown"],
        "audience": ["user", "agent"],
        "retry": "Never resend a payment when executed is \"yes\" or \"unknown\": keep the Idempotency-Key and read the account. Resend once, after retryAfterSecs (1 to 60 s), only on rate_limited or idempotency_pending with executed \"no\". Every other code needs the change nextStep names first.",
        "recentUpdates": recent_updates(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique_snake_case_and_well_formed() {
        let mut seen = std::collections::HashSet::new();
        for c in CODES {
            assert!(seen.insert(c.name), "{} twice", c.name);
            assert!(
                !c.name.is_empty()
                    && c.name
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b == b'_' || b.is_ascii_digit())
                    && !c.name.starts_with('_')
                    && !c.name.ends_with('_'),
                "{} is not snake_case",
                c.name
            );
            assert!(BY.contains(&c.by), "{}: by {}", c.name, c.by);
            assert!(
                matches!(c.audience, "user" | "agent"),
                "{}: audience {}",
                c.name,
                c.audience
            );
            assert!(
                !c.meaning.is_empty() && !c.next_step.is_empty(),
                "{}",
                c.name
            );
        }
        // docs/115 §2's starting list (`service_paused`: the demo's relayer floor, `demo::may_open`).
        for name in [
            "service_paused",
            "token_missing",
            "token_invalid",
            "token_scope",
            "token_stopped",
            "mode_watch",
            "mode_propose",
            "payee_not_approved",
            "payee_cooling_off",
            "over_per_payment",
            "over_24h",
            "below_floor",
            "needs_owner",
            "idempotency_conflict",
            "idempotency_pending",
            "invalid_body",
            "body_too_large",
            "rate_limited",
            "chain_unavailable",
            "upstream_unavailable",
            "price_out_of_band",
            "x402_unsupported",
        ] {
            assert!(code(name).is_some(), "{name} missing");
        }
    }

    #[test]
    fn a_refusal_is_exactly_the_shared_contract() {
        let r = refuse(
            "payee_not_approved",
            Executed::No,
            "0x6a86…c8EE is not on the account's payee list.",
            "Ask the owner to approve the payee; the ask is in Needs you.",
            "the sentence the route already returned, for a human",
            None,
        );
        assert_eq!(
            r,
            json!({ "ok": false, "contractVersion": "2026-10-17.1",
                "error": { "code": "payee_not_approved", "by": "payee", "executed": "no", "audience": "user",
                    "meaning": "0x6a86…c8EE is not on the account's payee list.",
                    "nextStep": "Ask the owner to approve the payee; the ask is in Needs you.",
                    "retryAfterSecs": 0, "words": "the sentence the route already returned, for a human" } })
        );
        let d = refuse("rate_limited", Executed::Unknown, "", " ", "busy", Some(7));
        assert_eq!(d["error"]["meaning"], code("rate_limited").unwrap().meaning);
        assert_eq!(
            d["error"]["nextStep"],
            code("rate_limited").unwrap().next_step
        );
        assert_eq!(d["error"]["executed"], "unknown");
        assert_eq!(d["error"]["retryAfterSecs"], 7);
        assert_eq!(
            ok(json!({ "id": 1 })),
            json!({ "id": 1, "ok": true, "contractVersion": CONTRACT_VERSION })
        );
    }

    #[test]
    #[should_panic(expected = "not in refusal::CODES")]
    fn an_unknown_code_panics_in_tests() {
        refuse("no_such_code", Executed::No, "", "", "", None);
    }

    #[test]
    fn a_route_sentence_becomes_the_envelope_and_a_page_gets_it_back() {
        let v = json!({ "error": "this account's agent is being registered now", "scope": "run" });
        let e = envelope(409, &v, false).unwrap();
        assert_eq!(e["ok"], false);
        assert_eq!(e["error"]["code"], "state_conflict");
        assert_eq!(e["error"]["executed"], "no");
        assert_eq!(
            e["error"]["words"],
            "this account's agent is being registered now"
        );
        assert_eq!(e["scope"], "run", "every other field is kept");
        assert_eq!(
            envelope(502, &json!({ "error": "x" }), true).unwrap()["error"]["executed"],
            "unknown",
            "a failure where money may move is never \"no\""
        );
        assert_eq!(
            envelope(429, &json!({ "error": "x" }), false).unwrap()["error"]["retryAfterSecs"],
            60
        );
        assert_eq!(
            envelope(200, &json!({ "id": 2 }), false).unwrap()["ok"],
            true
        );
        assert_eq!(envelope(409, &e, false), None, "already in the envelope");
        assert_eq!(envelope(200, &json!([1]), false), None);
        assert!(from_page(
            "GET /x HTTP/1.1\r\nSec-Fetch-Site: same-origin\r\nSec-Fetch-Mode: cors\r\n\r\n"
        ));
        assert!(
            !from_page("GET /x HTTP/1.1\r\nsec-fetch-mode: cors\r\nuser-agent: node\r\n\r\n"),
            "Node's fetch is no page (P-274)"
        );
        assert!(!from_page(
            "POST /x HTTP/1.1\r\nHost: h\r\n\r\n{\"a\":\"\r\nsec-fetch-site: x\"}"
        ));
        let page = for_page(&e).unwrap();
        assert_eq!(
            page["error"],
            "this account's agent is being registered now"
        );
        assert_eq!(page["scope"], "run");
        assert_eq!(for_page(&json!({ "error": "plain" })), None);
        for s in [400, 401, 403, 404, 409, 413, 422, 429, 500, 502, 503] {
            assert!(code(from_status(s)).is_some(), "{s}");
        }
    }

    #[test]
    fn the_agent_routes_are_the_tool_routes_and_the_keyed_ones() {
        for (m, p) in [
            ("GET", "/api/level1/inbox"),
            ("POST", "/api/level1/run"),
            ("POST", "/api/level1/buy"),
            ("POST", "/api/level1/bills"),
            ("POST", "/api/level1/prepare-payees"),
            ("POST", "/api/level1/prepare-renew"),
            ("GET", "/api/level1/purchases.jsonl"),
            ("POST", "/api/level1/invoice"),
            ("GET", "/api/level1/records"),
            ("GET", "/api/level1/record"),
            ("POST", "/api/watch"),
            ("GET", "/api/watch/6acfc3a5-0001"),
            ("GET", "/api/level1/codes"),
            ("GET", "/api/level1/status"),
        ] {
            assert!(agent_route(m, p), "{m} {p}");
        }
        for (m, p) in [
            ("POST", "/api/level1/deploy"),
            ("POST", "/api/level1/token-signed"),
            ("POST", "/api/level1/prepare-token"),
            ("GET", "/api/level1/config"),
            ("POST", "/api/desk/milestone"),
        ] {
            assert!(!agent_route(m, p), "{m} {p}");
        }
    }

    #[test]
    fn every_agent_key_refusal_has_its_code() {
        use crate::level1::{AgentToken, Tenant, token_allows, token_hash};
        let mut t: Tenant = serde_json::from_value(json!({
            "chain": "arbitrum-sepolia", "owner": "0x0000000000000000000000000000000000000001",
            "policy": "0x0000000000000000000000000000000000000002", "agent": "0x0000000000000000000000000000000000000003",
            "operator": "0x0000000000000000000000000000000000000004", "watched": "0x0000000000000000000000000000000000000005",
            "recovery": "0x0000000000000000000000000000000000000000", "per_action": "1", "daily": "1", "floor": "0",
            "delay_secs": 60, "mode": 2, "created_block": 0, "history_from_block": 0, "created": 0
        }))
        .unwrap();
        let w = |t: &Tenant, b: Option<&str>, s: &str| {
            token_code(token_allows(t, b, s, 10).unwrap_err())
        };
        assert_eq!(
            w(&t, None, "buy"),
            "token_scope",
            "no key minted: buying waits for the owner"
        );
        t.agent_tokens.push(
            serde_json::from_value::<AgentToken>(json!({
                "id": "k", "label": "l", "scopes": ["read"], "expires": 5, "issued": 0, "hash": token_hash("old")
            }))
            .unwrap(),
        );
        t.agent_tokens.push(
            serde_json::from_value::<AgentToken>(json!({
                "id": "k2", "label": "l", "scopes": ["read"], "expires": 99, "issued": 0, "hash": token_hash("good")
            }))
            .unwrap(),
        );
        assert_eq!(w(&t, None, "read"), "token_missing");
        assert_eq!(w(&t, Some("wrong"), "read"), "token_invalid");
        assert_eq!(w(&t, Some("old"), "read"), "token_expired");
        assert_eq!(w(&t, Some("good"), "buy"), "token_scope");
        // docs/60 P-251: a key the owner's device stopped.
        crate::guard::stop_token(
            &std::env::temp_dir().join(format!("refusal-stop-{}", std::process::id())),
            &t.policy,
            "k2",
        )
        .unwrap();
        assert_eq!(w(&t, Some("good"), "read"), "token_stopped");
    }

    /// Every code a site in this crate names literally is in the catalogue
    /// and in `llms.txt` (a typo would otherwise surface only at run time).
    #[test]
    fn every_code_a_site_names_is_catalogued_and_documented() {
        let llms = include_str!("../../../apps/classic/llms.txt");
        for c in CODES {
            assert!(
                llms.contains(&format!("`{}`", c.name)),
                "llms.txt lacks `{}`",
                c.name
            );
        }
        let sources = [
            include_str!("bin/tameion-app.rs"),
            include_str!("mcp.rs"),
            include_str!("pay.rs"),
            include_str!("x402.rs"),
        ];
        // The code is the first snake_case literal among a call's arguments
        // (`refuse("…"`, `refused("…"`, x402's `no("409 Conflict", "…"`).
        let mut named = 0;
        for src in sources {
            for pat in ["refuse(", "refused(", "no("] {
                for part in src.split(pat).skip(1) {
                    let args = part.split(')').next().unwrap_or("");
                    let name = args.split('"').skip(1).step_by(2).find(|l| {
                        l.contains('_') && l.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
                    });
                    if let Some(name) = name {
                        named += 1;
                        assert!(
                            code(name).is_some(),
                            "a site refuses with {name}, not in CODES"
                        );
                    }
                }
            }
        }
        assert!(named > 15, "the scan found the sites ({named})");
        assert!(recent_updates().as_array().is_some_and(|u| !u.is_empty()));
        assert_eq!(catalogue()["codes"].as_array().unwrap().len(), CODES.len());
    }
}
