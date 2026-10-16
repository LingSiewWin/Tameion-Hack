//! An interactive demo server: press a button, the agent actually runs.
//!
//! The static write-up of a past run proves nothing -- numbers in a page are
//! just numbers. This serves three endpoints instead:
//!
//!   GET  /            the page
//!   POST /api/run     runs one real pass and returns what happened
//!   GET  /api/verify  re-reads the chain and reports what *it* says
//!
//! `/api/verify` is the part that matters. It does not echo the run's own
//! claims back; it asks the contracts directly and returns their answers, so
//! the page can put the agent's record and the chain's record side by side
//! and let the reader see them match. A fabricated run fails that check.
//!
//! Every pass reasons in chain time. A second press straight after a first
//! pays nothing, because those billing cycles are settled on chain and the
//! next ones are not yet due -- the page shows that rather than inventing
//! new bills. (Until 2026-10-02 every press paid again: the agent learned
//! from its own payments and paid itself a month forward each time. That was
//! the defect, not the demo.) Advance the chain to see the next cycle.
//!
//! No web framework: two routes do not justify a dependency tree, and the
//! server is a demo harness, not something that faces the internet.
//!
//!   scripts/demo.sh

use std::sync::Arc as StdArc;

use alloy::primitives::{Address, B256};
use arc::ArcClient;
use ledger::{
    ActorKind, Direction, Money, Obligation, Prioritised, Seconds, Timestamp, ordering_divergence,
    rank_by_urgency,
};
use operator::claude::ClaudeReasoner;
use operator::hedge_advice;
use operator::reasoner::{Reasoner, ReasonerError};
use operator::{Config, Operator};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn env(k: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| panic!("{k} not set"))
}

/// A reasoner that hands back an ordering already computed.
///
/// The page shows the model's ordering next to the deterministic one, and the
/// loop then executes that same ordering. Without this the loop would call
/// the API a second time for an answer already in hand -- paying twice, and
/// risking the executed ordering differing from the displayed one.
struct Precomputed {
    /// `Err` when the model failed. Handing the error to the operator rather
    /// than resolving it here matters: the operator's fallback path is what
    /// writes the failure into the anchored log. Swallowing it in the server
    /// produced a run that fell back silently, with nothing in the record to
    /// say the model had been unavailable.
    result: Result<Vec<Prioritised>, ReasonerError>,
    name: String,
    kind: ActorKind,
}

impl Reasoner for Precomputed {
    async fn prioritise(
        &self,
        obligations: &[Obligation],
        _balance: Money,
        _now: Timestamp,
    ) -> Result<Vec<Prioritised>, ReasonerError> {
        match &self.result {
            Err(e) => Err(ReasonerError::Transient(e.to_string())),
            Ok(order) if order.len() != obligations.len() => Err(ReasonerError::WrongPopulation),
            Ok(order) => Ok(order.clone()),
        }
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn kind(&self) -> ActorKind {
        self.kind
    }
}

fn order_json(items: &[Prioritised]) -> Value {
    Value::Array(
        items
            .iter()
            .map(|p| {
                json!({
                    "who": p.obligation.counterparty.to_string(),
                    "amount": p.obligation.typical_amount.to_string(),
                    "why": p.rationale,
                })
            })
            .collect(),
    )
}

struct Ctx {
    rpc: String,
    agent_key: String,
    policy: Address,
    anchor: Address,
    /// Whose payment history is read. The treasury made the payments; the
    /// agent only proposes new ones.
    treasury: Address,
    /// Stand-in USDC. On Arc this is the native emitter instead.
    token: Address,
}

/// One real pass. Returns what happened, step by step, with timings.
async fn run_once(ctx: &Ctx, fixed_now: Option<i64>) -> Result<Value, Box<dyn std::error::Error>> {
    let t0 = std::time::Instant::now();
    let client = ArcClient::with_signer(&ctx.rpc, &ctx.agent_key)?.with_token(ctx.token);
    let agent = client.address().expect("signer");

    // Chain time, not wall time. The history was written on chain and every
    // due date is derived from it, so reasoning against the host's clock
    // compares dates from two unrelated timelines -- which is exactly the
    // bug that made a freshly-seeded demo report nothing due.
    let now = match fixed_now {
        Some(t) => Timestamp::from_unix(t),
        None => {
            let head = client.block_number().await?;
            client.block_timestamp(head).await?
        }
    };

    // Read it off the chain. This is the claim the whole project rests on --
    // recover the obligations from raw movements, with nobody tagging
    // anything -- so the demo must not shortcut it with a fabricated array.
    let config_probe = Config::new(ctx.treasury, ctx.policy, ctx.anchor);
    let probe = Operator::new(
        ArcClient::with_signer(&ctx.rpc, &ctx.agent_key)?.with_token(ctx.token),
        config_probe,
        Precomputed {
            result: Ok(vec![]),
            name: "probe".into(),
            kind: ActorKind::Rule,
        },
    )?;
    let history = probe.read_history().await?;
    let t_read = t0.elapsed().as_millis();

    // Infer, then order -- both done here so the page can show the model's
    // ordering beside the control before anything is spent. Inferred by the
    // operator's own computation, so the page shows exactly the obligations
    // the pass will act on: the agent's own payments are not evidence, paid
    // cycles are stepped over, and only what is within the pay lead is due.
    // (Computing this separately once let the demo order bills the pass had
    // already paid; the pass now refuses any ordering that is not a
    // permutation of its own due list.)
    let plan = probe.obligations_due(&history, now).await?;
    let upcoming_for_cover = plan.upcoming.clone();
    let obligations = plan.obligations;
    let due: Vec<Obligation> = plan.due;
    let t_infer = t0.elapsed().as_millis();

    let balance = client.policy_balance(ctx.policy).await?;
    let baseline = rank_by_urgency(&due, balance, now);

    let (result, reasoner_name, reasoner_kind, model_note) = match ClaudeReasoner::from_env() {
        Some(c) => match c.prioritise(&due, balance, now).await {
            Ok(o) => (Ok(o), c.name().to_string(), ActorKind::Model, Value::Null),
            Err(e) => {
                let note = json!(format!(
                    "Claude unavailable ({e}); the operator falls back."
                ));
                (Err(e), c.name().to_string(), ActorKind::Model, note)
            }
        },
        None => (
            Ok(baseline.clone()),
            "rule-based".to_string(),
            ActorKind::Rule,
            json!("No ANTHROPIC_API_KEY set; ran the deterministic control."),
        ),
    };
    let chosen = result
        .as_ref()
        .cloned()
        .unwrap_or_else(|_| baseline.clone());
    let t_decide = t0.elapsed().as_millis();
    let divergence = ordering_divergence(&baseline, &chosen);

    let mut config = Config::new(ctx.treasury, ctx.policy, ctx.anchor);
    config.horizon = Seconds::days(120).unwrap();
    let config_deadline_days = config.escalation_deadline.as_days();
    let mut op = Operator::new(
        client,
        config,
        Precomputed {
            result,
            name: reasoner_name.clone(),
            kind: reasoner_kind,
        },
    )?;

    // The hedge step runs first so its events sit under the same root the
    // pass anchors. Only when HEDGE_SPOT is set; dry run unless HEDGE_LIVE=1
    // and an API-wallet key exists. See `hedge::driver`.
    // docs/46 #4: the hedge may trade live only in Autopilot. Below that, a
    // live key is demoted to a dry run for this pass and the panel says so.
    // A level that could not be read is not Autopilot (audit 2026-10-15).
    let autopilot = operator::hedge_may_trade_live(op.client_ref(), ctx.policy).await;
    // docs/50 #2: with HEDGE_FROM_CHAIN=1 and no HEDGE_SPOT, the ETH held on
    // the mainnet presets is the exposure.
    let hedge_env = match hedge::driver::from_env(ctx.treasury) {
        Some(r) => Some(r),
        None if std::env::var("HEDGE_FROM_CHAIN").is_ok_and(|v| v == "1") => {
            let read = operator::watch::eth_holdings(ctx.treasury).await;
            let total: hedge::Decimal = read.iter().filter_map(|(_, r)| r.as_ref().ok()).sum();
            Some(hedge::driver::from_holdings(
                ctx.treasury,
                vec![hedge::engine::Holding {
                    coin: "ETH".into(),
                    size: total,
                }],
            ))
        }
        None => None,
    };
    // docs/50 #3: size the short to the bills the pass sees, not the holding,
    // unless HEDGE_COVER_USDC says otherwise.
    let cover: Option<hedge::Decimal> = hedge::driver::cover_follows_obligations().then(|| {
        upcoming_for_cover
            .iter()
            .map(|o| hedge::Decimal::from_i128_with_scale(o.typical_amount.to_minor(), 6))
            .sum()
    });
    let hedge_json = match hedge_env {
        None => Value::Null,
        Some(Err(e)) => json!({ "error": e }),
        Some(Ok(mut henv)) => {
            if henv.input.cover_usdc.is_none() {
                henv.input.cover_usdc = cover;
            }
            let demoted = !autopilot && matches!(henv.mode, hedge::engine::Mode::Live(_));
            if demoted {
                henv.mode = hedge::engine::Mode::DryRun;
            }
            let stepped = hedge::driver::step(&henv).await.map(|r| (r, demoted));
            match stepped {
                Err(e) => json!({ "error": e.to_string() }),
                Ok(((report, books), demoted)) => {
                    for body in hedge::record::events_for(&report)? {
                        op.log_mut().append(now, body);
                    }
                    // The model's seat: one proposal per coin, logged and
                    // attributed, applied by nobody. Only with a key.
                    let proposals: Vec<Value> = match ClaudeReasoner::from_env() {
                        None => Vec::new(),
                        Some(c) => {
                            match hedge_advice::propose(&c, &report, &henv.input.policy).await {
                                Ok(ps) => ps
                                    .iter()
                                    .map(|p| {
                                        op.log_mut().append(
                                            now,
                                            hedge_advice::event_for(p, hedge_advice::model_name()),
                                        );
                                        hedge_advice::json(p, hedge_advice::model_name())
                                    })
                                    .collect(),
                                Err(e) => vec![json!({ "error": e.to_string() })],
                            }
                        }
                    };
                    let mut hj = hedge::record::json(&report, &books, henv.mode_label());
                    hj["proposals"] = Value::Array(proposals);
                    hj["liveDemoted"] = json!(demoted);
                    let pol = &henv.input.policy;
                    hj["policy"] = json!({
                        "bandBps": pol.band_bps,
                        "maxLeverage": pol.max_leverage,
                        "kMaintenance": pol.k_maintenance,
                        "maxMarginSharePct": pol.max_margin_share_pct,
                        "allowlist": pol.allowlist,
                    });
                    hj
                }
            }
        }
    };

    let outcome = op.run_once(now, Some(history.clone()), true).await?;
    let t_done = t0.elapsed().as_millis();

    let settled: Vec<Value> = outcome
        .settled
        .iter()
        .map(|(o, tx)| {
            json!({
                "who": o.counterparty.to_string(),
                "amount": o.typical_amount.to_string(),
                "tx": format!("{tx:#x}"),
            })
        })
        .collect();

    let escalated: Vec<Value> = outcome
        .escalated
        .iter()
        .map(|(o, v)| {
            json!({
                "who": o.counterparty.to_string(),
                "amount": o.typical_amount.to_string(),
                // Structured, not prose. A consumer branches on `code`; `detail`
                // is for a person to read. Joining these into one English
                // sentence was forcing anything downstream to parse it back out.
                "checks": v.problems().map(|c| json!({
                    "code": c.name,
                    "outcome": c.outcome.as_str(),
                    "detail": c.detail,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();

    let hashes: Vec<Value> = operator::settlement_hashes(op.log())
        .into_iter()
        .map(|(cycle, h)| json!({ "cycle": cycle, "hash": h.to_string() }))
        .collect();

    Ok(json!({
        "agent": format!("{agent:#x}"),
        "now": now.as_unix(),
        "balance": balance.to_string(),
        "transfers": history.len(),
        "transfersFromChain": true,
        // Counterparties seen versus obligations claimed. The gap is the
        // point: an agent that turns every repeated address into a bill is
        // not inferring, it is guessing, and the refusals are the evidence
        // that it is doing the harder thing.
        "counterparties": history.iter()
            .filter(|t| t.direction == Direction::Out)
            .map(|t| t.counterparty.to_string())
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        "outgoing": history.iter().filter(|t| t.direction == Direction::Out).count(),
        "obligations": obligations.len(),
        "due": due.len(),
        // What actually reasoned, taken from the outcome rather than from the
        // intent: if the model failed, the operator's fallback ran and the
        // outcome is the honest record of that.
        "reasoner": outcome.reasoner,
        "reasonerKind": if outcome.reasoner.starts_with("rule-based") { "rule".to_string() } else { reasoner_kind.to_string() },
        "modelNote": model_note,
        "divergence": divergence,
        "orderRule": order_json(&baseline),
        "orderChosen": order_json(&chosen),
        "settled": settled,
        "escalated": escalated,
        "blocked": outcome.blocked.len(),
        "staleEscalations": outcome.stale_escalations,
        // Ready to sign: the agent composed these but the contract forbids it
        // from sending them.
        "pendingSignature": outcome.pending_signature.iter().map(|p| json!({
            "cycleKey": p.cycle_key,
            "counterparty": p.counterparty,
            "amount": p.amount.to_string(),
            "actionHash": p.action_hash,
            "settlementKey": p.settlement_key,
            "stale": p.stale,
            "to": format!("{:#x}", p.call.to),
            "data": p.call.data,
            "value": p.call.value.to_string(),
            "summary": p.call.summary,
            "attest": p.attest.as_ref().map(|a| json!({
                "to": format!("{:#x}", a.to),
                "data": a.data,
                "summary": a.summary,
            })),
        })).collect::<Vec<_>>(),
        "escalationDeadlineDays": config_deadline_days,
        "root": outcome.root.to_string(),
        "anchorTx": outcome.anchor_tx.map(|t| format!("{t:#x}")),
        // docs/46 #4: Watch / Propose / Autopilot, as the contract held it.
        "mode": outcome.mode.map(|m| m.label().to_string()).unwrap_or_else(|| "Autopilot (policy predates modes)".into()),
        "settlementHashes": hashes,
        // The decision log is the product, not a side effect: ship the record
        // itself rather than a summary of it.
        "log": operator::replay(op.log()),
        // Null when HEDGE_SPOT is unset. Otherwise the hedge step that ran
        // before the payments, under the same root.
        "hedge": hedge_json,
        "timings": { "read": t_read, "infer": t_infer, "decide": t_decide, "settle": t_done },
    }))
}

/// Ask the chain what it holds. Deliberately independent of the run: this
/// opens its own client and reads the contracts, so a match between these
/// numbers and the run's is evidence rather than an echo.
async fn verify(
    ctx: &Ctx,
    root_hex: &str,
    hashes: &[String],
) -> Result<Value, Box<dyn std::error::Error>> {
    let client = ArcClient::with_signer(&ctx.rpc, &ctx.agent_key)?;
    let agent = client.address().expect("signer");

    let onchain_root = client.anchored_root(ctx.anchor, agent).await?;
    let onchain_root_hex = format!("{onchain_root:#x}");

    let mut spent = Vec::new();
    for h in hashes {
        let parsed: B256 = h.parse()?;
        let ok = client.already_executed(ctx.policy, parsed).await?;
        spent.push(json!({ "hash": h, "executed": ok }));
    }

    let authority = client.authority(ctx.policy).await?;
    let balance = client.policy_balance(ctx.policy).await?;
    // docs/46 #2: the most a leaked agent key can lose in one window, from
    // chain reads only -- min(dailyCap - spent, balance - reserveFloor).
    let governance = client.governance(ctx.policy).await.ok();
    let floor = governance
        .as_ref()
        .map(|g| g.reserve_floor.to_minor())
        .unwrap_or(0);
    let room = (authority.daily_cap.to_minor() - authority.spent_today.to_minor()).max(0);
    let reserve_room = (balance.to_minor() - floor).max(0);
    let max_loss =
        ledger::money::Money::minor(room.min(reserve_room), ledger::money::Currency::Usdc);
    Ok(json!({
        "agent": format!("{agent:#x}"),
        "chainRoot": onchain_root_hex,
        "agentRoot": root_hex,
        "rootsMatch": onchain_root_hex.eq_ignore_ascii_case(root_hex),
        "spent": spent,
        "perActionCap": authority.per_action_cap.to_string(),
        "dailyCap": authority.daily_cap.to_string(),
        "policyBalance": balance.to_string(),
        "mode": governance.as_ref().map(|g| g.mode.label()),
        "delaySecs": governance.as_ref().map(|g| g.delay_secs),
        "reserveFloor": governance.as_ref().map(|g| g.reserve_floor.to_string()),
        "pending": governance.as_ref().and_then(|g| g.pending.as_ref()).map(|p| json!({
            "kinds": p.kinds,
            "perActionCap": p.per_action_cap.to_string(),
            "dailyCap": p.daily_cap.to_string(),
            "reserveFloor": p.reserve_floor.to_string(),
            "mode": p.mode.label(),
            "eta": p.eta_unix,
        })),
        "maxLossAgent24h": max_loss.to_string(),
    }))
}

fn respond(status: &str, ctype: &str, body: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    out.extend_from_slice(body);
    out
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ctx = StdArc::new(Ctx {
        rpc: env("RPC_URL"),
        agent_key: env("AGENT_PK"),
        policy: env("POLICY").parse()?,
        anchor: env("ANCHOR").parse()?,
        treasury: env("TREASURY").parse()?,
        token: env("TOKEN").parse()?,
    });

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(7878);
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    println!("\n  Tameion demo → http://127.0.0.1:{port}\n");
    println!("  policy {}  anchor {}", ctx.policy, ctx.anchor);
    println!(
        "  reasoner {}\n",
        if std::env::var("ANTHROPIC_API_KEY").is_ok() {
            "claude-opus-5"
        } else {
            "rule-based (no key)"
        }
    );

    loop {
        let (mut sock, _) = listener.accept().await?;
        let ctx = StdArc::clone(&ctx);
        tokio::spawn(async move {
            let mut buf = vec![0u8; 16 * 1024];
            let n = match sock.read(&mut buf).await {
                Ok(n) if n > 0 => n,
                _ => return,
            };
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let line = req.lines().next().unwrap_or("");
            let mut parts = line.split_whitespace();
            let method = parts.next().unwrap_or("");
            let target = parts.next().unwrap_or("/");
            let path = target.split('?').next().unwrap_or("/");
            let body = req.split("\r\n\r\n").nth(1).unwrap_or("").to_string();

            let reply = match (method, path) {
                ("GET", "/") => respond(
                    "200 OK",
                    "text/html; charset=utf-8",
                    include_str!("serve.html").as_bytes(),
                ),
                ("POST", "/api/run") => {
                    let fixed = serde_json::from_str::<Value>(&body)
                        .ok()
                        .and_then(|v| v["now"].as_i64());
                    match run_once(&ctx, fixed).await {
                        Ok(v) => respond("200 OK", "application/json", v.to_string().as_bytes()),
                        Err(e) => respond(
                            "500 Internal Server Error",
                            "application/json",
                            json!({ "error": e.to_string() }).to_string().as_bytes(),
                        ),
                    }
                }
                ("POST", "/api/verify") => {
                    let parsed: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                    let root = parsed["root"].as_str().unwrap_or("").to_string();
                    let hashes: Vec<String> = parsed["hashes"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    match verify(&ctx, &root, &hashes).await {
                        Ok(v) => respond("200 OK", "application/json", v.to_string().as_bytes()),
                        Err(e) => respond(
                            "500 Internal Server Error",
                            "application/json",
                            json!({ "error": e.to_string() }).to_string().as_bytes(),
                        ),
                    }
                }
                _ => respond("404 Not Found", "text/plain", b"not found"),
            };

            let _ = sock.write_all(&reply).await;
            let _ = sock.flush().await;
        });
    }
}
