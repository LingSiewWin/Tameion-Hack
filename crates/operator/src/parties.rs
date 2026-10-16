//! Who is on the other side of a payment, said the way a person says it
//! (docs/81). The account page, the agent's reports and the MCP tools name
//! parties through [`parties`]; a person never has to read `0x…` — the full
//! address stays in the party's details, for the explorer and for agents.
//!
//! Where a name comes from, strongest first: the owner named it (from the
//! device that opened the account, never an agent key); it is one of this
//! account's own (its wallets, spare key, agent key); someone outside proved
//! it (another Tameion account's handle, a domain, an ENS name — `verified`,
//! found by the resolvers); the API it was bought from (x402: that site
//! answered with this pay-to address); what the payee signed into its own
//! invoice (claimed, not checked). With none of these the party is described
//! by what the account knows: "every 30 days · 500 USDC", "never paid".
//!
//! A claimed name that is already the name of a different address is a
//! clash, shown as one — "Landlord" on a stranger's address is how invoice
//! fraud works, and a look-alike address is how address poisoning works.

use crate::level1::Tenant;
use alloy::primitives::Address;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::str::FromStr;

/// A name the owner gave an address.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Contact {
    pub address: String,
    pub name: String,
    pub at: u64,
}

pub const MAX_CONTACTS: usize = 500;
pub const MAX_NAME: usize = 40;

/// A name the owner may give: NFKC, no invisible format characters (Cf: zero
/// width spaces and joiners, direction marks, BOM — docs/103 S2-2), spaces
/// collapsed, 1–40 characters, no control characters, and nothing that reads
/// as an address (a name "0x6c1c…9a25" would pass for one). The console cleans
/// a counterparty's name the same way before it is signed
/// (`apps/console/src/lib/names.ts` `cleanName`), so the signed bytes are these.
pub fn clean_name(s: &str) -> Result<String, String> {
    use icu_properties::{CodePointMapData, props::GeneralCategory};
    let gc = CodePointMapData::<GeneralCategory>::new();
    let nfkc = icu_normalizer::ComposingNormalizerBorrowed::new_nfkc().normalize(s);
    let visible: String = nfkc
        .chars()
        .filter(|c| gc.get(*c) != GeneralCategory::Format)
        .collect();
    let n = visible.split_whitespace().collect::<Vec<_>>().join(" ");
    if n.is_empty() {
        return Err("a name cannot be empty".into());
    }
    if n.chars().count() > MAX_NAME {
        return Err(format!("a name is at most {MAX_NAME} characters"));
    }
    if n.chars().any(char::is_control) {
        return Err("a name cannot hold control characters".into());
    }
    if n.to_ascii_lowercase().starts_with("0x") {
        return Err("a name cannot look like an address".into());
    }
    Ok(n)
}

/// What two names are compared by (docs/103 S2-2): names that read alike are
/// one name. NFKD, so accents come apart from their letters; no format
/// characters and no combining marks (Cf, Mn); the Cyrillic and Greek letters
/// that pass for Latin ones, and 0 / 1 / capital I, folded to the Latin letter
/// they pass for; lower case; spaces collapsed.
/// Every check that refuses a second key under a name compares this, never
/// the raw string. ponytail: a hand-kept table of the look-alikes that matter
/// for Latin names (UTS #39 lists thousands); the `unicode-security` crate's
/// `skeleton` is the upgrade if names in other scripts start to collide.
pub fn name_key(s: &str) -> String {
    use icu_properties::{CodePointMapData, props::GeneralCategory};
    let gc = CodePointMapData::<GeneralCategory>::new();
    let nfkd = icu_normalizer::DecomposingNormalizerBorrowed::new_nfkd().normalize(s);
    let folded = nfkd
        .chars()
        .flat_map(|c| {
            look_alike(c).map_or_else(|| c.to_lowercase().collect::<Vec<_>>(), |l| vec![l])
        })
        .filter(|c| {
            !matches!(
                gc.get(*c),
                GeneralCategory::Format | GeneralCategory::NonspacingMark
            )
        })
        .collect::<String>();
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The Latin letter a character passes for in a Latin name, upper or lower case.
fn look_alike(c: char) -> Option<char> {
    Some(match c {
        // Cyrillic
        'А' | 'а' => 'a',
        'В' | 'в' => 'b',
        'Е' | 'е' | 'Ё' | 'ё' => 'e',
        'Һ' | 'һ' | 'Н' => 'h',
        'І' | 'і' | 'Ї' | 'ї' => 'i',
        'Ј' | 'ј' => 'j',
        'К' | 'к' => 'k',
        'Ӏ' | 'ӏ' => 'l',
        'М' | 'м' => 'm',
        'О' | 'о' => 'o',
        'Р' | 'р' => 'p',
        'Ԛ' | 'ԛ' => 'q',
        'С' | 'с' => 'c',
        'Ѕ' | 'ѕ' => 's',
        'Т' | 'т' => 't',
        'У' | 'у' | 'Ү' | 'ү' => 'y',
        'Ԝ' | 'ԝ' => 'w',
        'Х' | 'х' => 'x',
        // Greek
        'Α' | 'α' => 'a',
        'Β' | 'β' => 'b',
        'Ε' => 'e',
        'Ζ' => 'z',
        'Η' => 'h',
        'Ι' | 'ι' => 'i',
        'Κ' | 'κ' => 'k',
        'Μ' => 'm',
        'Ν' => 'n',
        'ν' => 'v',
        'Ο' | 'ο' => 'o',
        'Ρ' | 'ρ' => 'p',
        'Τ' | 'τ' => 't',
        'Υ' => 'y',
        'υ' => 'u',
        'Χ' | 'χ' => 'x',
        // Latin and digits that pass for each other
        'ı' => 'i',
        'I' | '1' | '|' => 'l',
        '0' => 'o',
        _ => return None,
    })
}

/// Name `a` (or forget its name, with an empty `name`) on `t`.
pub fn set_contact(t: &mut Tenant, a: Address, name: &str, now: u64) -> Result<(), String> {
    let key = format!("{a:#x}");
    t.contacts.retain(|c| !c.address.eq_ignore_ascii_case(&key));
    if name.trim().is_empty() {
        return Ok(());
    }
    if t.contacts.len() >= MAX_CONTACTS {
        return Err(format!("{MAX_CONTACTS} names at most; forget one first"));
    }
    t.contacts.push(Contact {
        address: key,
        name: clean_name(name)?,
        at: now,
    });
    Ok(())
}

/// Names proved from outside the account, by address: the name and how it
/// was proved (`"tameion"`, `"domain"`, `"ens"`).
pub type Verified = BTreeMap<Address, (String, &'static str)>;

/// What the inbox says about one address: the cadence and amount of a bill to
/// it, how often it was paid, the site it was bought from.
#[derive(Debug, Default, Clone)]
struct Facts {
    every_secs: Option<u64>,
    amount: Option<String>,
    paid: u32,
    last_paid: Option<u64>,
    host: Option<String>,
    seller: bool,
}

const PARTY_KEYS: [&str; 3] = ["counterparty", "payee", "seller"];

fn addr(v: &Value) -> Option<Address> {
    v.as_str()
        .and_then(|s| Address::from_str(s.trim()).ok())
        .filter(|a| !a.is_zero())
}

/// Every party the reply names, with what the rows around it say.
fn walk(v: &Value, out: &mut BTreeMap<Address, Facts>) {
    match v {
        Value::Object(o) => {
            for k in PARTY_KEYS {
                let Some(a) = o.get(k).and_then(addr) else {
                    continue;
                };
                let f = out.entry(a).or_default();
                f.seller |= k == "seller";
                if let Some(e) = o
                    .get("intervalSecs")
                    .or_else(|| o.get("everySecs"))
                    .and_then(Value::as_u64)
                    .filter(|e| *e > 0)
                {
                    f.every_secs.get_or_insert(e);
                }
                // Amounts come in two shapes across the reply; only the
                // spelled-out one ("0.5 USDC") is unambiguous.
                if let Some(m) = o
                    .get("amount")
                    .and_then(Value::as_str)
                    .filter(|m| m.ends_with(" USDC"))
                {
                    f.amount.get_or_insert_with(|| crate::push::human(m));
                }
                if o.get("tx").is_some_and(Value::is_string)
                    && let Some(at) = o.get("at").and_then(Value::as_u64)
                {
                    f.paid += 1;
                    f.last_paid = Some(f.last_paid.map_or(at, |l| l.max(at)));
                }
                if let Some(h) = o
                    .get("bought")
                    .or_else(|| o.get("host"))
                    .and_then(Value::as_str)
                    .filter(|h| !h.is_empty())
                {
                    f.host.get_or_insert_with(|| h.to_string());
                }
            }
            for x in o.values() {
                walk(x, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
        _ => {}
    }
}

fn span(secs: u64) -> String {
    let (n, unit) = match secs {
        s if s >= 86_400 => ((s + 43_200) / 86_400, "day"),
        s if s >= 3_600 => ((s + 1_800) / 3_600, "hour"),
        s if s >= 60 => ((s + 30) / 60, "minute"),
        s => (s, "second"),
    };
    if n == 1 {
        format!("1 {unit}")
    } else {
        format!("{n} {unit}s")
    }
}

/// The relation, in words: what is owed and how often, else how often it was
/// paid, else that it never was.
fn note(f: &Facts, now: u64) -> String {
    if let Some(e) = f.every_secs {
        let s = span(e);
        // "every 1 day" reads as "every day".
        let every = match s.strip_prefix("1 ") {
            Some(unit) => format!("every {unit}"),
            None => format!("every {s}"),
        };
        return match &f.amount {
            Some(m) => format!("{m} {every}"),
            None => every,
        };
    }
    if f.paid > 0 {
        let ago = f
            .last_paid
            .map(|l| now.saturating_sub(l))
            .map(|d| {
                if d < 3_600 {
                    "within the hour".to_string()
                } else {
                    format!("{} ago", span(d))
                }
            })
            .unwrap_or_default();
        let times = if f.paid == 1 {
            "paid once".to_string()
        } else {
            format!("paid {} times", f.paid)
        };
        return if ago.is_empty() {
            times
        } else {
            format!("{times} · last {ago}")
        };
    }
    "never paid".into()
}

/// The last four hex digits, the way a card shows its last four.
pub fn tail(a: Address) -> String {
    let h = format!("{a:x}");
    h[h.len() - 4..].to_ascii_uppercase()
}

/// The account's own addresses, in words.
fn own(t: &Tenant) -> Vec<(Address, String, &'static str)> {
    let mut out: Vec<(Address, String, &'static str)> = Vec::new();
    let mut push = |a: Option<Address>, label: String, note: &'static str| {
        if let Some(a) = a.filter(|a| !a.is_zero())
            && !out.iter().any(|(x, _, _)| *x == a)
        {
            out.push((a, label, note));
        }
    };
    let parse = |s: &str| Address::from_str(s.trim()).ok();
    push(
        parse(&t.policy),
        "This account".into(),
        "where the money is held",
    );
    let many = t.wallets.len() > 1;
    for (i, w) in t.wallets.iter().enumerate() {
        let label = if many {
            format!("Your wallet {}", i + 1)
        } else {
            "Your wallet".into()
        };
        push(parse(w), label, "a wallet you connected");
    }
    push(
        parse(&t.watched),
        "Your bill wallet".into(),
        "the wallet this account reads",
    );
    push(
        parse(&t.recovery),
        "Your spare key".into(),
        "can name a new passkey if you lose yours",
    );
    push(
        t.spare_key.as_deref().and_then(parse),
        "Your spare key".into(),
        "proposed: confirm it with your passkey",
    );
    push(
        parse(&t.owner),
        "Your passkey".into(),
        "the account's owner",
    );
    if !t.keyless {
        push(
            parse(&t.agent),
            "The agent's key".into(),
            "pays approved payees under your limit",
        );
    }
    push(
        parse(&t.operator),
        "Tameion's relayer".into(),
        "pays the gas; cannot move your money",
    );
    out
}

/// What each payee signed about itself in its newest version-2 invoice:
/// `(from, domain)`, by payee.
pub fn claims(invoices: &[Value]) -> BTreeMap<Address, (String, String)> {
    let mut claimed: BTreeMap<Address, (String, String)> = BTreeMap::new();
    for v in invoices {
        let Some(a) = addr(&v["payee"]) else { continue };
        let from = v["from"].as_str().unwrap_or("").trim().to_string();
        let domain = v["domain"].as_str().unwrap_or("").trim().to_string();
        if (!from.is_empty() || !domain.is_empty()) && !claimed.contains_key(&a) {
            claimed.insert(a, (from, domain));
        }
    }
    claimed
}

/// Every party address `body` names (the rows [`parties`] reads).
pub fn named_in(body: &Value) -> Vec<Address> {
    let mut f = BTreeMap::new();
    walk(body, &mut f);
    f.into_keys().collect()
}

/// Every party named in `body` (the inbox reply), plus this account's own
/// addresses and the owner's names, as `{address: {label, kind, verified,
/// note, tail, domain?}}`. `private_ok`: the owner's names go only to the
/// owner's device (or an agent key the gate already checked). `invoices`:
/// the account's invoice lines, whose `from`/`domain` the payee signed.
pub fn parties(
    t: &Tenant,
    body: &Value,
    invoices: &[Value],
    verified: &Verified,
    private_ok: bool,
    now: u64,
) -> Value {
    let mut facts: BTreeMap<Address, Facts> = BTreeMap::new();
    walk(body, &mut facts);
    let mine = own(t);
    for (a, _, _) in &mine {
        facts.entry(*a).or_default();
    }
    let all_names: BTreeMap<Address, String> = t
        .contacts
        .iter()
        .filter_map(|c| Some((Address::from_str(&c.address).ok()?, c.name.clone())))
        .collect();
    let owner_names = if private_ok {
        all_names.clone()
    } else {
        BTreeMap::new()
    };
    for a in owner_names.keys() {
        facts.entry(*a).or_default();
    }
    let claimed = claims(invoices);
    for a in claimed.keys() {
        facts.entry(*a).or_default();
    }
    // Names already bound to an address: a claim of one of them by another
    // address is a clash. Every owner name counts here, shown or not: a
    // co-owner on a device without the token must see "Not your Hosting" too
    // (the claimed name is the sender's own words, so nothing private leaks
    // beyond "a payee by that name exists").
    let mut bound: BTreeMap<String, Address> = BTreeMap::new();
    for (a, n) in all_names.iter() {
        bound.entry(n.to_lowercase()).or_insert(*a);
    }
    for (a, (n, _)) in verified {
        bound.entry(n.to_lowercase()).or_insert(*a);
    }
    for (a, f) in &facts {
        if let Some(h) = &f.host {
            bound.entry(h.to_lowercase()).or_insert(*a);
        }
    }
    let mut out = Map::new();
    for (a, f) in &facts {
        let rel = note(f, now);
        let mut p = if let Some(n) = owner_names.get(a) {
            json!({ "label": n, "kind": "owner", "verified": true, "note": rel })
        } else if let Some((_, label, why)) = mine.iter().find(|(x, _, _)| x == a) {
            json!({ "label": label, "kind": "you", "verified": true, "note": why })
        } else if let Some((n, how)) = verified.get(a) {
            json!({ "label": n, "kind": how, "verified": true, "note": rel })
        } else if let Some(h) = &f.host {
            json!({ "label": h, "kind": "x402", "verified": true, "note": rel })
        } else if let Some((from, domain)) = claimed.get(a) {
            let said = if from.is_empty() { domain } else { from };
            let other = [from, domain]
                .into_iter()
                .filter(|s| !s.is_empty())
                .find_map(|s| bound.get(&s.to_lowercase()).filter(|b| *b != a));
            match other {
                Some(_) => json!({
                    "label": format!("Not your {said}"),
                    "kind": "clash",
                    "verified": false,
                    "note": format!("its invoice says it is {said}; the {said} you know is a different account"),
                }),
                None => json!({
                    "label": said,
                    "kind": "invoice",
                    "verified": false,
                    "note": rel,
                }),
            }
        } else {
            let role = if f.seller { "Seller" } else { "Payee" };
            json!({ "label": format!("{role} ••{}", tail(*a)), "kind": "new", "verified": false, "note": rel })
        };
        p["tail"] = json!(tail(*a));
        if let Some((from, d)) = claimed.get(a) {
            if !d.is_empty() {
                p["domain"] = json!(d);
            }
            // What it calls itself, beside a name proved another way.
            if !from.is_empty() && p["label"] != json!(from) {
                p["from"] = json!(from);
            }
        }
        out.insert(format!("{a:#x}"), p);
    }
    Value::Object(out)
}

// ---- Names proved outside the account (docs/81 phase B) ----------------------
//
// Three proofs, each one the party's own act, none bought from us:
// - another account on this service, by the handle it chose (`acme`);
// - its web domain: the domain's DNS lists the address (an ERC-7529 TXT record,
//   `ERC-7529.<chainId>._domaincontracts.<domain>`, Draft, read here for any
//   address, not only contracts), and the payee signed that domain into its own
//   invoice — both sides say it;
// - its ENS primary name, read through ENS's Universal Resolver, which returns
//   a name only if the name resolves back to the same address.
// Every lookup is cached and time-boxed: a slow name never holds up the page.

/// ENS's Universal Resolver on Ethereum mainnet (docs.ens.domains/resolvers/universal).
pub const ENS_UNIVERSAL_RESOLVER: &str = "0xeEeEEEeE14D718C2B47D9923Deab1335E144EeEe";
/// Multicall3, the same address on every EVM chain: one read for many names.
pub const MULTICALL3: &str = "0xcA11bde05977b3631167028862bE2a173976CA11";
const DOH: &str = "https://cloudflare-dns.com/dns-query";
const FOUND_SECS: u64 = 86_400;
const MISSED_SECS: u64 = 6 * 3_600;

alloy::sol! {
    function reverse(bytes lookupAddress, uint256 coinType) external view returns (string primary, address resolver, address reverseResolver);
    function resolve(bytes name, bytes data) external view returns (bytes result, address resolver);
    function addr(bytes32 node) external view returns (address);
    struct Call3 { address target; bool allowFailure; bytes callData; }
    struct Result3 { bool success; bytes returnData; }
    function aggregate3(Call3[] calls) external payable returns (Result3[] returnData);
}

type Cache<V> = std::sync::Mutex<BTreeMap<String, (V, u64)>>;
static ENS: std::sync::LazyLock<Cache<Option<String>>> = std::sync::LazyLock::new(Default::default);
static TXT: std::sync::LazyLock<Cache<Vec<Address>>> = std::sync::LazyLock::new(Default::default);

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn cached<V: Clone>(c: &Cache<V>, k: &str) -> Option<V> {
    let m = c.lock().unwrap_or_else(|e| e.into_inner());
    m.get(k)
        .filter(|(_, until)| *until > now())
        .map(|(v, _)| v.clone())
}

fn keep<V>(c: &Cache<V>, k: String, v: V, secs: u64) {
    let mut m = c.lock().unwrap_or_else(|e| e.into_inner());
    // ponytail: unbounded until a restart; an LRU when one service names millions of addresses.
    m.insert(k, (v, now() + secs));
}

/// An ENS name a person can read without being fooled: lowercase ASCII
/// letters, digits, `-`, `_` and dots. ponytail: names with other characters
/// (emoji, other scripts) are dropped rather than shown, since a look-alike
/// letter is the point of an impersonation; ENSIP-15 normalisation and a
/// confusables check would let them through.
pub fn plain_ens(n: &str) -> bool {
    n.len() <= 255
        && n.contains('.')
        && n.split('.').all(|l| !l.is_empty())
        && n.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_.".contains(&b))
}

fn ens_client() -> Option<arc::ArcClient> {
    let c = crate::watch::chain("ethereum")?;
    arc::ArcClient::read_only(&crate::watch::rpc_for(c)).ok()
}

/// ENS primary names for `addrs`, one batched read on Ethereum mainnet.
/// `None` for an address without one (cached too); an address the read did
/// not reach is left out, to be tried on the next call.
pub async fn ens_names(addrs: &[Address]) -> BTreeMap<Address, Option<String>> {
    use alloy::sol_types::SolCall;
    let mut out = BTreeMap::new();
    let mut todo = Vec::new();
    for a in addrs {
        match cached(&ENS, &format!("{a:#x}")) {
            Some(v) => {
                out.insert(*a, v);
            }
            None => todo.push(*a),
        }
    }
    let (Some(client), Ok(ur), Ok(mc)) = (
        ens_client(),
        Address::from_str(ENS_UNIVERSAL_RESOLVER),
        Address::from_str(MULTICALL3),
    ) else {
        return out;
    };
    for chunk in todo.chunks(40) {
        let calls = chunk
            .iter()
            .map(|a| Call3 {
                target: ur,
                allowFailure: true,
                // coinType 60: the Ethereum address record (ENSIP-11).
                callData: reverseCall {
                    lookupAddress: a.to_vec().into(),
                    coinType: alloy::primitives::U256::from(60u8),
                }
                .abi_encode()
                .into(),
            })
            .collect();
        let data = aggregate3Call { calls }.abi_encode();
        let read = tokio::time::timeout(
            std::time::Duration::from_secs(4),
            client.call_raw(None, mc, data),
        )
        .await;
        let Ok(Ok(raw)) = read else { continue };
        let Ok(results) = aggregate3Call::abi_decode_returns(&raw) else {
            continue;
        };
        for (a, r) in chunk.iter().zip(results) {
            // A revert is "no verified name" (the resolver checks the name
            // resolves back to this address and reverts when it does not).
            let name = r
                .success
                .then(|| reverseCall::abi_decode_returns(&r.returnData).ok())
                .flatten()
                .map(|x| x.primary)
                .filter(|n| plain_ens(n));
            keep(
                &ENS,
                format!("{a:#x}"),
                name.clone(),
                if name.is_some() {
                    FOUND_SECS
                } else {
                    MISSED_SECS
                },
            );
            out.insert(*a, name);
        }
    }
    out
}

/// The addresses `domain` lists for `chain_id` in DNS (ERC-7529 record name),
/// over DNS-over-HTTPS. Empty when it lists none; `None` when DNS did not answer.
pub async fn domain_addresses(chain_id: u64, domain: &str) -> Option<Vec<Address>> {
    if !crate::invoice::valid_domain(domain) {
        return Some(Vec::new());
    }
    let key = format!("{chain_id}:{domain}");
    if let Some(v) = cached(&TXT, &key) {
        return Some(v);
    }
    let name = format!("ERC-7529.{chain_id}._domaincontracts.{domain}");
    let r = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(4))
        .build()
        .ok()?
        // `name` is checked letters, digits, dots and hyphens: no escaping needed.
        .get(format!("{DOH}?name={name}&type=TXT"))
        .header("accept", "application/dns-json")
        .send()
        .await
        .ok()?
        .json::<Value>()
        .await
        .ok()?;
    let found = txt_addresses(&r);
    keep(
        &TXT,
        key,
        found.clone(),
        if found.is_empty() {
            MISSED_SECS
        } else {
            FOUND_SECS
        },
    );
    Some(found)
}

/// The addresses in a DoH JSON answer's TXT records: each record's quoted
/// strings joined, then split on commas (ERC-7529's VALUE).
fn txt_addresses(r: &Value) -> Vec<Address> {
    let mut out = Vec::new();
    for a in r["Answer"].as_array().into_iter().flatten() {
        if a["type"].as_u64() != Some(16) {
            continue;
        }
        let joined: String = a["data"]
            .as_str()
            .unwrap_or("")
            .split('"')
            .enumerate()
            .filter(|(i, _)| i % 2 == 1)
            .map(|(_, s)| s)
            .collect();
        for x in joined.split(',') {
            if let Ok(addr) = Address::from_str(x.trim())
                && !out.contains(&addr)
            {
                out.push(addr);
            }
        }
    }
    out
}

/// The names proved for `addrs`: another account's handle on this service,
/// else the domain the payee signed into its invoice when that domain's DNS
/// lists it, else its ENS primary name.
pub async fn proved(
    app_dir: &std::path::Path,
    chain_id: u64,
    addrs: &[Address],
    claimed: &BTreeMap<Address, (String, String)>,
) -> Verified {
    let mut out = Verified::new();
    let handles: BTreeMap<Address, String> = crate::level1::list(app_dir)
        .into_iter()
        .filter_map(|t| Some((Address::from_str(&t.policy).ok()?, t.name?)))
        .collect();
    let mut rest = Vec::new();
    for a in addrs {
        match handles.get(a) {
            Some(h) => {
                out.insert(*a, (h.clone(), "tameion"));
            }
            None => rest.push(*a),
        }
    }
    let domains: Vec<(Address, String)> = rest
        .iter()
        .filter_map(|a| {
            let (_, d) = claimed.get(a)?;
            (!d.is_empty()).then(|| (*a, d.clone()))
        })
        .collect();
    let mut set = tokio::task::JoinSet::new();
    for (a, d) in domains {
        set.spawn(async move { (a, d.clone(), domain_addresses(chain_id, &d).await) });
    }
    let ens = ens_names(&rest).await;
    while let Some(Ok((a, d, listed))) = set.join_next().await {
        if listed.is_some_and(|l| l.contains(&a)) {
            out.insert(a, (d, "domain"));
        }
    }
    for (a, n) in ens {
        if let Some(n) = n {
            out.entry(a).or_insert((n, "ens"));
        }
    }
    out
}

/// ENS namehash (EIP-137).
pub fn namehash(name: &str) -> alloy::primitives::B256 {
    let mut node = alloy::primitives::B256::ZERO;
    for label in name.rsplit('.').filter(|l| !l.is_empty()) {
        node = alloy::primitives::keccak256(
            [
                node.as_slice(),
                alloy::primitives::keccak256(label.as_bytes()).as_slice(),
            ]
            .concat(),
        );
    }
    node
}

/// DNS wire encoding of a name (length-prefixed labels, then a zero byte).
pub fn dns_encode(name: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for l in name.split('.').filter(|l| !l.is_empty()) {
        out.push(l.len().min(255) as u8);
        out.extend_from_slice(&l.as_bytes()[..l.len().min(255)]);
    }
    out.push(0);
    out
}

/// What a person typed for a payee — a Tameion handle, an ENS name, a web
/// domain or an address — as the address it stands for and the name to show.
/// A domain must list exactly one address for `chain_id`; an ENS name must
/// resolve on Ethereum mainnet.
pub async fn resolve_typed(
    app_dir: &std::path::Path,
    chain_id: u64,
    q: &str,
) -> Result<(Address, String, &'static str), String> {
    use alloy::sol_types::SolCall;
    let q = q.trim().to_lowercase();
    if let Ok(a) = Address::from_str(&q) {
        return Ok((a, format!("••{}", tail(a)), "address"));
    }
    if crate::level1::valid_name(&q) {
        let t = crate::level1::by_name(app_dir, &q)
            .into_iter()
            .next()
            .ok_or(format!("no Tameion account is called {q}"))?;
        let a = Address::from_str(&t.policy).map_err(|e| e.to_string())?;
        return Ok((a, q, "tameion"));
    }
    if !plain_ens(&q) {
        return Err("type a name (acme), an ENS name (alice.eth), a web domain (acme.com) or a wallet address".into());
    }
    // An ENS name first (alice.eth, acme.base.eth, or a DNS name imported into ENS).
    let ur = Address::from_str(ENS_UNIVERSAL_RESOLVER).map_err(|e| e.to_string())?;
    if let Some(client) = ens_client() {
        let call = resolveCall {
            name: dns_encode(&q).into(),
            data: addrCall { node: namehash(&q) }.abi_encode().into(),
        }
        .abi_encode();
        if let Ok(Ok(raw)) = tokio::time::timeout(
            std::time::Duration::from_secs(4),
            client.call_raw(None, ur, call),
        )
        .await
            && let Ok(r) = resolveCall::abi_decode_returns(&raw)
            && let Ok(a) = addrCall::abi_decode_returns(&r.result)
            && !a.is_zero()
        {
            return Ok((a, q, "ens"));
        }
    }
    // Then the domain's own list.
    match domain_addresses(chain_id, &q).await {
        Some(l) if l.len() == 1 => Ok((l[0], q, "domain")),
        Some(l) if l.len() > 1 => Err(format!(
            "{q} lists {} addresses; type the one you mean",
            l.len()
        )),
        Some(_) => Err(format!(
            "{q} is neither an ENS name nor a domain that lists an address for this chain"
        )),
        None => Err("the name lookup did not answer; try again or type the address".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_that_reads_like_another_is_the_same_name() {
        // docs/103 S2-2, the reviewer's own name: "Lim Brothers Gold" + U+200B was accepted as a second name.
        let real = "Lim Brothers Gold";
        let zwsp = "Lim Brothers Gold\u{200B}";
        assert_eq!(
            clean_name(zwsp).unwrap(),
            real,
            "the invisible character does not survive cleaning"
        );
        assert_eq!(name_key(zwsp), name_key(real));
        for alike in [
            "Lim Brothers Gold\u{200D}",              // zero width joiner
            "\u{202E}Lim Brothers Gold",              // a direction override
            "Lim Br\u{043E}thers Gold",               // Cyrillic о
            "Lim Broth\u{0435}rs Gold",               // Cyrillic е
            "Lim Brothers G\u{039F}ld",               // Greek Ο
            "Lim Brothers GoId",                      // capital I for l
            "Lim Brothers G0ld",                      // zero for o
            "Lim Brothers Go\u{0323}ld",              // a combining dot below
            "\u{FF2C}\u{FF49}\u{FF4D} Brothers Gold", // fullwidth letters
            "lim  brothers   GOLD",                   // case and spaces
        ] {
            assert_eq!(name_key(alike), name_key(real), "{alike:?}");
        }
        for other in [
            "Lim Brothers Silver",
            "Lim Brother Gold",
            "Harbour Vault KL",
            "金城珠宝",
        ] {
            assert_ne!(name_key(other), name_key(real), "{other:?}");
        }
        assert_eq!(clean_name("\u{FF2C}im  Brothers").unwrap(), "Lim Brothers");
        assert_eq!(clean_name("金城珠宝").unwrap(), "金城珠宝");
        assert!(clean_name("\u{200B}\u{200B}").is_err());
    }

    fn tenant() -> Tenant {
        serde_json::from_value(json!({
            "chain": "arbitrum-sepolia", "owner": "0x00000000000000000000000000000000000000a1",
            "policy": "0x00000000000000000000000000000000000000b2", "agent": "0x00000000000000000000000000000000000000c3",
            "operator": "0x00000000000000000000000000000000000000d4", "watched": "0x0000000000000000000000000000000000000000",
            "recovery": "0x0000000000000000000000000000000000000000", "per_action": "50", "daily": "100", "floor": "0",
            "delay_secs": 60, "mode": 1, "created_block": 0, "history_from_block": 0, "created": 0,
            "owner_tx": null, "policy_tx": null,
            "wallets": ["0x00000000000000000000000000000000000000e5"]
        }))
        .unwrap()
    }

    const ALICE: &str = "0x1111111111111111111111111111111111119a25";
    const MALLORY: &str = "0x2222222222222222222222222222222222229a25";
    const STRANGER: &str = "0x3333333333333333333333333333333333330b0b";

    #[test]
    fn names_by_strength_and_never_hex() {
        let mut t = tenant();
        let alice = Address::from_str(ALICE).unwrap();
        set_contact(&mut t, alice, "  Alice   (design) ", 1).unwrap();
        let body = json!({
            "standingOrders": [{ "payee": ALICE, "amount": "500 USDC", "everySecs": 2_592_000 }],
            "paid": [{ "counterparty": STRANGER, "amount": "1 USDC", "tx": "0xab", "at": 1_000 },
                     { "counterparty": STRANGER, "amount": "1 USDC", "tx": "0xcd", "at": 90_000 }],
            "purchases": [{ "counterparty": "0x4444444444444444444444444444444444444444", "bought": "api.weather.example", "tx": "0xef", "at": 5 }],
        });
        let inv =
            vec![json!({ "payee": MALLORY, "from": "Alice (design)", "domain": "alice.example" })];
        let p = parties(&t, &body, &inv, &Verified::new(), true, 100_000);
        let get = |a: &str| p[a.to_lowercase()].clone();
        assert_eq!(get(ALICE)["label"], "Alice (design)");
        assert_eq!(get(ALICE)["kind"], "owner");
        assert_eq!(get(ALICE)["note"], "500 USDC every 30 days");
        // The same name signed by another address is a clash, not the name.
        assert_eq!(get(MALLORY)["kind"], "clash");
        assert_eq!(get(MALLORY)["label"], "Not your Alice (design)");
        assert_eq!(get(MALLORY)["tail"], "9A25");
        // Unnamed: described, last four only.
        assert_eq!(get(STRANGER)["label"], "Payee ••0B0B");
        assert_eq!(get(STRANGER)["note"], "paid 2 times · last 3 hours ago");
        assert_eq!(
            get("0x4444444444444444444444444444444444444444")["label"],
            "api.weather.example"
        );
        assert_eq!(
            get("0x00000000000000000000000000000000000000b2")["label"],
            "This account"
        );
        assert_eq!(
            get("0x00000000000000000000000000000000000000e5")["label"],
            "Your wallet"
        );
        // No label anywhere reads as an address.
        for (_, v) in p.as_object().unwrap() {
            assert!(!v["label"].as_str().unwrap().contains("0x"), "{v}");
        }
        // Without the owner's device, the owner's names stay home.
        let q = parties(&t, &body, &inv, &Verified::new(), false, 100_000);
        assert_eq!(q[ALICE.to_lowercase()]["kind"], "new");
        // ...but a claim of one is still a clash on every device.
        assert_eq!(q[MALLORY.to_lowercase()]["kind"], "clash");
    }

    #[test]
    fn claimed_names_without_a_clash_say_so() {
        let t = tenant();
        let inv = vec![json!({ "payee": MALLORY, "from": "", "domain": "acme.example" })];
        let p = parties(&t, &json!({}), &inv, &Verified::new(), true, 0);
        let m = &p[MALLORY.to_lowercase()];
        assert_eq!(m["label"], "acme.example");
        assert_eq!(m["kind"], "invoice");
        assert_eq!(m["verified"], false);
        assert_eq!(m["domain"], "acme.example");
        // How the name is known is `kind`; `note` is only the relation.
        assert_eq!(m["note"], "never paid");
    }

    #[test]
    fn owner_names_are_clean() {
        assert!(clean_name("0xdeadbeef").is_err());
        assert!(clean_name("   ").is_err());
        assert!(clean_name(&"a".repeat(41)).is_err());
        assert!(clean_name("Land\u{7}lord").is_err());
        assert_eq!(
            clean_name(" Rent  for the office ").unwrap(),
            "Rent for the office"
        );
        let mut t = tenant();
        let a = Address::from_str(ALICE).unwrap();
        set_contact(&mut t, a, "Alice", 1).unwrap();
        set_contact(&mut t, a, "Alice B", 2).unwrap();
        assert_eq!(t.contacts.len(), 1);
        set_contact(&mut t, a, "", 3).unwrap();
        assert!(t.contacts.is_empty());
    }

    #[test]
    fn dns_answers_and_ens_helpers() {
        let r = json!({ "Answer": [
            { "type": 16, "data": "\"0x1111111111111111111111111111111111119a25,0x2222222222222222222222222222222222229a25\"" },
            { "type": 16, "data": "\"0x33333333333333333333\" \"33333333333333330b0b\"" },
            { "type": 5, "data": "\"0x4444444444444444444444444444444444444444\"" },
        ]});
        let l = txt_addresses(&r);
        assert_eq!(l.len(), 3, "{l:?}");
        assert_eq!(l[2], Address::from_str(STRANGER).unwrap());
        // EIP-137's own example values.
        assert_eq!(namehash(""), alloy::primitives::B256::ZERO);
        assert_eq!(
            format!("{:#x}", namehash("eth")),
            "0x93cdeb708b7545dc668eb9280176169d1c33cfd8ed6f04690a0bcc88a93fc4ae"
        );
        assert_eq!(
            format!("{:#x}", namehash("foo.eth")),
            "0xde9b09fd7c5f901e23a3f19fecc54828e9c848539801e86591bd9801b019f84f"
        );
        assert_eq!(dns_encode("alice.eth"), b"\x05alice\x03eth\x00".to_vec());
        assert!(plain_ens("vitalik.eth") && plain_ens("acme.base.eth"));
        assert!(!plain_ens("vitаlik.eth"), "a Cyrillic а is not shown");
        assert!(!plain_ens("Vitalik.eth") && !plain_ens("eth") && !plain_ens("a..eth"));
    }
}
