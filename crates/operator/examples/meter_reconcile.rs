//! Our token meter against Anthropic's usage report (docs/117 §6).
//!
//!     cargo run -p operator --example meter_reconcile                  # the last 60 minutes, live
//!     cargo run -p operator --example meter_reconcile -- --minutes 30
//!     cargo run -p operator --example meter_reconcile -- \
//!       --report crates/operator/tests/fixtures/anthropic-usage-report.json \
//!       --keys crates/operator/tests/fixtures/anthropic-api-keys.json \
//!       --meter crates/operator/tests/fixtures/meter-synthetic.jsonl  # a saved report page
//!     ANTHROPIC_API_KEY="$(cat ~/.tameion/anthropic.key)" \
//!       cargo run -p operator --example meter_reconcile -- --probe     # one real decider call
//!
//! Live mode reads the Admin key from `TAMEION_ANTHROPIC_ADMIN_KEY_FILE` (default
//! `~/.tameion/anthropic-admin.key`): the report is Admin API only, and a Messages key is
//! refused. The meter is `<app dir>/meter.jsonl` (`TAMEION_APP_DIR`, default `runtime/app`)
//! unless `--meter` names another file. `--probe` asks the decider about the first golden
//! scenario through a metered client and prints the line it wrote (never the key hint).
//! Exit 0: every bucket agrees; 1: a bucket differs; 2: the run could not happen.

use std::path::{Path, PathBuf};

use operator::meter::{self, Bucket, Key, Keys, Report};
use operator::statement::iso8601;

fn fail(why: &str) -> ! {
    eprintln!("meter_reconcile: {why}");
    std::process::exit(2)
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let flag = |k: &str| {
        args.iter()
            .position(|a| a == k)
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
    };
    let app_dir =
        PathBuf::from(std::env::var("TAMEION_APP_DIR").unwrap_or_else(|_| "runtime/app".into()));
    let meter_path = flag("--meter").map_or_else(|| app_dir.join("meter.jsonl"), PathBuf::from);
    if args.iter().any(|a| a == "--probe") {
        return probe(&meter_path).await;
    }
    let (buckets, keys) = match flag("--report") {
        Some(report) => offline(report, flag("--keys")),
        None => {
            let minutes = flag("--minutes").map_or(60, |m| {
                m.parse()
                    .unwrap_or_else(|_| fail("--minutes takes a whole number"))
            });
            live(minutes).await
        }
    };
    let ours = meter::read(&meter_path).unwrap_or_else(|e| fail(&e.to_string()));
    let checks = meter::reconcile(&ours, &buckets, &keys).unwrap_or_else(|e| fail(&e));
    println!(
        "meter {} ({} lines) against {} report buckets",
        meter_path.display(),
        ours.len(),
        buckets.len()
    );
    for c in &checks {
        println!(
            "{} .. {}  {:<17} ours {:>6} in {:>5} out | theirs {:>6} in {:>5} out | {}",
            iso8601(c.from),
            iso8601(c.to),
            c.model,
            c.ours_in,
            c.ours_out,
            c.theirs_in,
            c.theirs_out,
            c.note
        );
    }
    let differs = checks
        .iter()
        .any(|c| c.class != meter::MATCH && c.class != meter::SUMMARY);
    std::process::exit(i32::from(differs));
}

/// A report page and a key list saved to files (the fixture run).
fn offline(report: &str, keys: Option<&str>) -> (Vec<Bucket>, Vec<Key>) {
    let read = |p: &str| std::fs::read_to_string(p).unwrap_or_else(|e| fail(&format!("{p}: {e}")));
    let r: Report = serde_json::from_str(&read(report))
        .unwrap_or_else(|e| fail(&format!("{report}: not the documented shape: {e}")));
    if r.has_more {
        println!("note: {report} is one page of several (has_more); only this page is compared");
    }
    let keys = keys.map_or_else(Vec::new, |p| {
        serde_json::from_str::<Keys>(&read(p))
            .unwrap_or_else(|e| fail(&format!("{p}: not the documented shape: {e}")))
            .data
    });
    (r.data, keys)
}

/// The last `minutes` whole minutes from Anthropic, with the organisation's key list.
async fn live(minutes: i64) -> (Vec<Bucket>, Vec<Key>) {
    let file = std::env::var("TAMEION_ANTHROPIC_ADMIN_KEY_FILE").unwrap_or_else(|_| {
        format!(
            "{}/.tameion/anthropic-admin.key",
            std::env::var("HOME").unwrap_or_default()
        )
    });
    let admin = match std::fs::read_to_string(&file) {
        Ok(k) if !k.trim().is_empty() => k.trim().to_string(),
        _ => {
            println!(
                "framed-not-built — missing: an Anthropic Admin API key at {file} \
                 (Console → Organization → Admin keys; it reads usage and cost, never sends a message)."
            );
            println!(
                "Recommended with it: a separate Messages key for the public host, so the report's \
                 api_key_id tells its traffic from this Mac's."
            );
            std::process::exit(2);
        }
    };
    let to = now() / 60 * 60;
    let from = to - minutes.max(1) * 60;
    let http = reqwest::Client::new();
    let base = operator::claude::base_url();
    let buckets = meter::fetch_report(&http, &base, &admin, from, to)
        .await
        .unwrap_or_else(|e| fail(&e));
    // Without the key list the comparison is organisation-wide, and every note says so.
    let keys = meter::fetch_keys(&http, &base, &admin)
        .await
        .unwrap_or_else(|e| {
            println!("note: no key list ({e}); comparing organisation-wide");
            Vec::new()
        });
    (buckets, keys)
}

/// One real decider call through a metered client: the capture half, end to end.
async fn probe(meter_path: &Path) {
    use operator::decide::{Case, Decider, ModelDecider};
    let Some(cloud) = operator::claude::ClaudeReasoner::from_env() else {
        fail("--probe makes one real call: set ANTHROPIC_API_KEY")
    };
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("../tests/fixtures/decide-scenarios.json"))
            .unwrap_or_else(|e| fail(&e.to_string()));
    let case: Case = serde_json::from_value(golden["scenarios"][0]["case"].clone())
        .unwrap_or_else(|e| fail(&format!("first golden scenario: {e}")));
    let before = meter::read(meter_path).map_or(0, |l| l.len());
    let decider = ModelDecider::new(cloud).with_meter(meter_path.to_path_buf(), None);
    match decider.decide(&case).await {
        Ok(d) => println!(
            "decided {} ({}) by {}: {}",
            d.action.as_str(),
            d.basis.as_str(),
            d.model.unwrap_or_default(),
            d.why
        ),
        Err(e) => {
            println!("the model did not decide ({e}); a billed reply is metered all the same")
        }
    }
    let lines = meter::read(meter_path).unwrap_or_else(|e| fail(&e.to_string()));
    for l in lines.iter().skip(before) {
        println!(
            "meter line: at {} model {} purpose {} uncached_in {} cache_read {} cache_create {} out {} request_id {}",
            iso8601(l.at),
            l.model,
            l.purpose,
            l.uncached_in,
            l.cache_read,
            l.cache_create,
            l.out,
            l.request_id.as_deref().unwrap_or("-")
        );
    }
    if lines.len() <= before {
        fail("no line was written: the reply carried no usage, or the meter file is not writable");
    }
}
