//! Fetch (or read from cache) the price table the service pins (docs/121) and
//! print it: `TAMEION_PRICES=live cargo run -p operator --example price_table -- <app_dir>`.
//! The merged table is saved as `<app_dir>/price/<hash>.json`;
//! `python3 -I scripts/price_hash.py` recomputes its hash from that file.
use std::time::{SystemTime, UNIX_EPOCH};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::path::PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("usage: price_table <app_dir>")?,
    );
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
    let t = operator::price::current(&dir, now)
        .await
        .ok_or("no table: set TAMEION_PRICES=live (or fixture)")?;
    println!("{:#}", t.to_json());
    println!(
        "saved {}",
        dir.join("price").join(format!("{}.json", t.hash)).display()
    );
    println!("note: {}", t.note());
    Ok(())
}
