use hedge::book;
use hedge::engine::Books;
use hedge::store::{self, JOURNAL_FILE, STATE_FILE};
use ledger::{Currency, Money};
use rust_decimal::dec;
use std::path::PathBuf;

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hedge-store-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn nothing_saved_means_none() {
    let d = temp("none");
    assert!(store::load_or_new(&d, &["ETH"]).unwrap().is_none());
}

#[test]
fn save_then_load_round_trips_balances_and_appends_only_new_entries() {
    let d = temp("roundtrip");
    let mut books = Books::new(&["ETH"], 1_700_000_000_000);
    books.journal.append(
        book::mark_spot("ETH", dec!(1250), "ETH@1")
            .unwrap()
            .unwrap(),
    );
    books.journal.append(
        book::mark_hedge("ETH", dec!(-12.5), "ETH@1")
            .unwrap()
            .unwrap(),
    );
    books
        .journal
        .append(book::funding("ETH", dec!(0.25), "0xabc").unwrap().unwrap());
    assert_eq!(books.unsaved(), 3);
    assert_eq!(store::save(&d, &mut books).unwrap(), 3);
    assert_eq!(books.unsaved(), 0);
    // Saving again writes nothing new.
    assert_eq!(store::save(&d, &mut books).unwrap(), 0);
    let lines = std::fs::read_to_string(d.join(JOURNAL_FILE)).unwrap();
    assert_eq!(lines.lines().count(), 3);

    let loaded = store::load_or_new(&d, &["ETH"]).unwrap().unwrap();
    assert_eq!(loaded.journal.len(), 3);
    assert_eq!(loaded.unsaved(), 0);
    assert_eq!(loaded.funding_cursor_ms, 1_700_000_000_000);
    let (dr, cr) = loaded.journal.trial_balance(Currency::Usdc).unwrap();
    assert_eq!(dr, cr);
    let v = book::net_view(&loaded.journal, &loaded.chart, "ETH").unwrap();
    assert_eq!(v.spot, Money::minor(1_250_000_000, Currency::Usdc));
    assert_eq!(v.hedge, Money::minor(-12_500_000, Currency::Usdc));
    assert_eq!(v.margin, Money::minor(250_000, Currency::Usdc));

    // A fourth entry on the loaded books appends exactly one line.
    let mut loaded = loaded;
    loaded
        .journal
        .append(book::fee(dec!(0.01), "fee-1").unwrap().unwrap());
    assert_eq!(store::save(&d, &mut loaded).unwrap(), 1);
    let lines = std::fs::read_to_string(d.join(JOURNAL_FILE)).unwrap();
    assert_eq!(lines.lines().count(), 4);
    assert!(d.join(STATE_FILE).exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_coin_booked_before_is_still_in_the_chart() {
    let d = temp("coins");
    let mut books = Books::new(&["SOL"], 0);
    books.journal.append(
        book::mark_spot("SOL", dec!(2160.2), "SOL@1")
            .unwrap()
            .unwrap(),
    );
    store::save(&d, &mut books).unwrap();
    // This run only cares about ETH, but the SOL balance must still answer.
    let loaded = store::load_or_new(&d, &["ETH"]).unwrap().unwrap();
    let v = book::net_view(&loaded.journal, &loaded.chart, "SOL").unwrap();
    assert_eq!(v.spot, Money::minor(2_160_200_000, Currency::Usdc));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_edited_line_is_refused_with_its_line_number() {
    let d = temp("corrupt");
    let mut books = Books::new(&["ETH"], 0);
    books
        .journal
        .append(book::mark_spot("ETH", dec!(100), "ETH@1").unwrap().unwrap());
    books
        .journal
        .append(book::mark_spot("ETH", dec!(50), "ETH@2").unwrap().unwrap());
    store::save(&d, &mut books).unwrap();
    // Someone changes the second entry's debit so it no longer balances.
    let p = d.join(JOURNAL_FILE);
    let text = std::fs::read_to_string(&p).unwrap();
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    lines[1] = lines[1].replacen("\"minor\":\"50000000\"", "\"minor\":\"50000001\"", 1);
    assert!(
        lines[1].contains("50000001"),
        "edit must have applied: {}",
        lines[1]
    );
    // Only the first occurrence (the debit) was changed, so the entry is unbalanced.
    std::fs::write(&p, lines.join("\n") + "\n").unwrap();
    let err = match store::load_or_new(&d, &["ETH"]) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("an unbalanced line must be refused"),
    };
    assert!(err.contains("line 2"), "{err}");
    assert!(err.contains("unbalanced"), "{err}");
    let _ = std::fs::remove_dir_all(&d);
}
