//! What the next pass of a hosted account would act on, read only (docs/60
//! P-210): the bills stepped past every cycle already paid, those due now, and
//! those set aside with why. Sends nothing and writes nothing.
//!
//! ```text
//! cargo run -p operator --example plan_tenant -- <app_dir> <policy>
//! ```

#[tokio::main]
async fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (Some(dir), Some(policy)) = (a.first(), a.get(1)) else {
        eprintln!("usage: plan_tenant <app_dir> <policy>");
        std::process::exit(2);
    };
    let dir = std::path::Path::new(dir);
    let Some(t) = operator::level1::load(dir, &policy.to_lowercase()) else {
        eprintln!("no such tenant in {}", dir.display());
        std::process::exit(1);
    };
    match operator::level1::plan(dir, &t).await {
        Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default()),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
