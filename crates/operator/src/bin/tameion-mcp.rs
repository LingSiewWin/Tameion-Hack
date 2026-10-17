//! Tameion as a tool for other agents (docs/54 §3, PLAN 5.11): a Model
//! Context Protocol server over stdio that a company's own agent — Claude
//! Code, OpenClaw, Hermes, anything that speaks MCP — plugs in to ask what
//! the treasury owes, run a pass, or prepare a payment.
//!
//! It is a thin, read-mostly client of the hosted service (`tameion-app`):
//! every tool maps to one HTTP route, and this binary holds no key. The tool
//! table and the JSON-RPC handling are `operator::mcp`, shared with the
//! service's `POST /mcp` (the same tools over Streamable HTTP).
//!
//! Protocol: JSON-RPC 2.0 over newline-delimited stdio, MCP 2025-06-18
//! subset — `initialize`, `notifications/initialized`, `ping`,
//! `tools/list`, `tools/call`.
//!
//! Environment:
//!   TAMEION_URL   the service, default http://127.0.0.1:7880
//!   TAMEION_TOKEN the agent key the owner minted on the account page (PLAN 5.31), sent as Bearer
//!   TAMEION_MCP_TIMEOUT_SECS how long a call waits for the service, default 60
//!
//! Claude Code: `claude mcp add tameion -- cargo run -q -p operator --bin tameion-mcp`
//! (or the built binary), then ask it what the treasury owes.

use operator::mcp;
use std::io::{BufRead, Write};

#[tokio::main]
async fn main() {
    let up = mcp::Upstream::from_env();
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(answer) = mcp::stdio_line(&up, &line).await {
            let _ = writeln!(out, "{answer}");
            let _ = out.flush();
        }
    }
}
