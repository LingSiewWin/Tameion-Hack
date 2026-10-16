"""Evidence desk: where did this money come from?

Reads every stablecoin payment into a set of addresses from public chain
explorers, gives each one a reason, values it at Bank Negara Malaysia's rate
for the day it arrived, and writes a source-of-funds pack that a bank or an
exchange's compliance desk can check line by line.

The same discipline as the payment engine (ledger::verify): every inflow gets
a verdict, and refusing is kept separate from asking.

- INCLUDED: the reason is known, from the earner or from the explorer.
- EXCLUDED: not income, and nobody needs to be asked: the earner's own
  transfer, dust, poisoning bait, a token that only calls itself USDC.
- ASK: the reason is not known. The pack lists these for the earner to
  confirm; it never guesses one.

Read-only. Holds no funds, signs nothing, sends nothing. Standard library only,
so it runs anywhere Python 3.10+ does.
"""

from __future__ import annotations

import hashlib
import json
import secrets
import ssl
import time
import urllib.error
import urllib.request
from dataclasses import asdict, dataclass, field
from datetime import date, datetime, timedelta, timezone
from decimal import Decimal

USER_AGENT = "tameion-evidence/0.1"

# Public Blockscout instances. No API key.
EXPLORERS = {
    "ethereum": "https://eth.blockscout.com",
    "arbitrum": "https://arbitrum.blockscout.com",
    "base": "https://base.blockscout.com",
    "optimism": "https://optimism.blockscout.com",
    "polygon": "https://polygon.blockscout.com",
    "arc-testnet": "https://explorer.testnet.arc.io",
}

# Stablecoins recognised by CONTRACT ADDRESS, never by symbol: anyone can
# deploy a token called "USDC". Each address was checked on its own explorer
# on 2026-10-01 (symbol, decimals, holders in the hundreds of thousands to
# millions); the check is reproduced by `pack.py --verify-tokens`.
STABLECOINS = {
    "ethereum": {
        "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48": ("USDC", 6),
        "0xdac17f958d2ee523a2206206994597c13d831ec7": ("USDT", 6),
    },
    "arbitrum": {
        "0xaf88d065e77c8cc2239327c5edb3a432268e5831": ("USDC", 6),
        "0xff970a61a04b1ca14834a43f5de4533ebddb5cc8": ("USDC.e", 6),
        "0xfd086bc7cd5c481dcc9c85ebe478a1c0b69fcbb9": ("USDT0", 6),
    },
    "base": {
        "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913": ("USDC", 6),
        "0xfde4c96c8593536e31f229ea8f37b2ada2699bb2": ("USDT", 6),
    },
    "optimism": {
        "0x0b2c639c533813f4aa9d7837caf62653d097ff85": ("USDC", 6),
        "0x94b008aa00579c1307b0ef2c499ad98a8ce58e58": ("USDT", 6),
    },
    "polygon": {
        "0x3c499c542cef5e3811e1192ce70d8cc03d5c3359": ("USDC", 6),
        "0xc2132d05d31c914a87c6611c10748aeb04b58e8f": ("USDT0", 6),
    },
    # Arc's USDC is native; there is no ERC-20 transfer to read here yet.
    "arc-testnet": {},
}

# A payment of one whole unit or less is dust: the threshold docs/21 §7 used,
# and the same one as ledger::is_dust.
DUST = Decimal("1")

# Address-poisoning look-alikes copy the first and last characters.
LOOKALIKE_PREFIX = 4
LOOKALIKE_SUFFIX = 4

# Explorer names and tags that mean "this left an exchange account".
EXCHANGE_WORDS = (
    "binance", "coinbase", "kraken", "okx", "bybit", "bitget", "kucoin",
    "gate.io", "htx", "huobi", "crypto.com", "luno", "hata", "upbit",
    "bitfinex", "mexc", "gemini", "bitstamp", "hyperliquid",
)

CATEGORIES = ("prize", "grant", "bounty", "salary", "client", "refund", "other")


@dataclass
class Inflow:
    chain: str
    tx_hash: str
    log_index: int
    timestamp: str  # ISO 8601, UTC
    token: str
    token_address: str
    amount: str  # decimal string, token units
    payer: str
    payer_name: str | None = None
    payer_tags: list[str] = field(default_factory=list)
    payer_is_contract: bool = False
    payer_flagged_scam: bool = False
    recipient: str = ""
    # filled by label()
    verdict: str = ""  # INCLUDED | EXCLUDED | ASK
    category: str = ""
    basis: str = ""
    # filled by value()
    rate_date: str | None = None
    usd_myr: str | None = None
    myr: str | None = None

    def link(self) -> str:
        return f"{EXPLORERS[self.chain]}/tx/{self.tx_hash}"


# --------------------------------------------------------------- fetching


def _get(url: str, headers: dict | None = None, tries: int = 5,
         context: ssl.SSLContext | None = None) -> dict:
    """GET JSON, backing off on throttling and transient server errors."""
    delay = 1.0
    last: Exception | None = None
    for _ in range(tries):
        req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT, **(headers or {})})
        try:
            with urllib.request.urlopen(req, timeout=60, context=context) as r:
                return json.load(r)
        except urllib.error.HTTPError as e:
            if e.code == 404:
                raise
            last = e
        except (urllib.error.URLError, TimeoutError) as e:
            last = e
        time.sleep(delay)
        delay = min(delay * 2, 16)
    raise RuntimeError(f"{url}: {last}")


def fetch_inflows(chain: str, address: str, since: date, until: date) -> list[Inflow]:
    """Every ERC-20 transfer INTO `address` on `chain` between the dates."""
    base = EXPLORERS[chain]
    url = f"{base}/api/v2/addresses/{address}/token-transfers?type=ERC-20&filter=to"
    out: list[Inflow] = []
    params: dict | None = None
    while True:
        page = url if not params else url + "&" + "&".join(f"{k}={v}" for k, v in params.items())
        d = _get(page)
        stop = False
        for it in d.get("items", []):
            ts = it.get("timestamp") or ""
            day = ts[:10]
            if day and day > until.isoformat():
                continue
            if day and day < since.isoformat():
                stop = True  # newest first: everything after this is older
                break
            tok = it.get("token") or {}
            total = it.get("total") or {}
            dec = int(total.get("decimals") or tok.get("decimals") or 0)
            raw = int(total.get("value") or 0)
            frm = it.get("from") or {}
            tags = [t.get("display_name") or t.get("label") or "" for t in (frm.get("public_tags") or [])]
            meta = frm.get("metadata") or {}
            tags += [t.get("name") or "" for t in (meta.get("tags") or [])]
            out.append(
                Inflow(
                    chain=chain,
                    tx_hash=it.get("transaction_hash") or it.get("tx_hash") or "",
                    log_index=int(it.get("log_index") or 0),
                    timestamp=ts,
                    token=tok.get("symbol") or "?",
                    token_address=(tok.get("address_hash") or tok.get("address") or "").lower(),
                    amount=str(Decimal(raw) / (Decimal(10) ** dec)) if dec else str(raw),
                    payer=(frm.get("hash") or "").lower(),
                    payer_name=frm.get("name") or frm.get("ens_domain_name"),
                    payer_tags=[t for t in tags if t],
                    payer_is_contract=bool(frm.get("is_contract")),
                    payer_flagged_scam=bool(frm.get("is_scam")),
                    recipient=address.lower(),
                )
            )
        params = d.get("next_page_params")
        if stop or not params:
            break
    return out


class Rates:
    """Bank Negara Malaysia's 17:00 USD middle rate, by business day.

    A payment that lands on a weekend or holiday is valued at the last
    business day before it, and the pack says which day was used.
    """

    API = "https://api.bnm.gov.my/public/exchange-rate/USD/year/{y}/month/{m}?session=1700&quote=rm"
    HEADERS = {"Accept": "application/vnd.BNM.API.v1+json"}

    @staticmethod
    def _bnm_get(url: str, headers: dict) -> dict:
        # api.bnm.gov.my negotiates only cipher suites that OpenSSL 3's
        # default security level 2 refuses (measured 2026-10-01: handshake
        # failure at the default, success at level 1; curl succeeds). The
        # certificate is still verified; only the cipher policy is relaxed,
        # and only for this read-only public endpoint.
        ctx = ssl.create_default_context()
        ctx.set_ciphers("DEFAULT@SECLEVEL=1")
        return _get(url, headers, context=ctx)

    def __init__(self, fetch=None):
        self._fetch = fetch or Rates._bnm_get
        self._months: dict[tuple[int, int], dict[str, Decimal]] = {}

    def _month(self, y: int, m: int) -> dict[str, Decimal]:
        key = (y, m)
        if key not in self._months:
            try:
                d = self._fetch(self.API.format(y=y, m=m), self.HEADERS)
                rows = d.get("data", {}).get("rate", [])
            except urllib.error.HTTPError:
                rows = []
            self._months[key] = {
                r["date"]: Decimal(str(r["middle_rate"])) for r in rows if r.get("middle_rate")
            }
        return self._months[key]

    def on_or_before(self, day: date) -> tuple[str, Decimal] | None:
        d = day
        for _ in range(40):  # long enough for any run of holidays
            rate = self._month(d.year, d.month).get(d.isoformat())
            if rate is not None:
                return d.isoformat(), rate
            d -= timedelta(days=1)
        return None


# --------------------------------------------------------------- labelling


def _body(addr: str) -> str | None:
    a = addr.lower()
    if a.startswith("0x") and len(a) == 42:
        return a[2:]
    return None


def imitates(candidate: str, real: str) -> bool:
    """Same first and last characters, different address."""
    c, r = _body(candidate), _body(real)
    if not c or not r or c == r:
        return False
    return c[:LOOKALIKE_PREFIX] == r[:LOOKALIKE_PREFIX] and c[-LOOKALIKE_SUFFIX:] == r[-LOOKALIKE_SUFFIX:]


def label(inflows: list[Inflow], own: set[str], payers: dict[str, dict]) -> list[Inflow]:
    """Give every inflow a verdict and a reason. Never invents a category.

    `own` is the earner's own addresses. `payers` maps a payer address to what
    the earner says it is: {"label": "ETHGlobal prize", "category": "prize"}.
    """
    own = {o.lower() for o in own}
    payers = {k.lower(): v for k, v in payers.items()}
    # Who genuinely pays this earner: anyone who sent more than dust, plus
    # anyone the earner has named. Look-alikes are judged against these.
    genuine = {i.payer for i in inflows if Decimal(i.amount) > DUST} | set(payers) | own

    for i in inflows:
        amount = Decimal(i.amount)
        known_token = STABLECOINS.get(i.chain, {}).get(i.token_address)
        twin = next((g for g in genuine if imitates(i.payer, g)), None)
        names = " ".join([i.payer_name or "", *i.payer_tags]).lower()
        stated = payers.get(i.payer)

        if known_token is None:
            i.verdict, i.category = "EXCLUDED", "not-a-stablecoin"
            i.basis = f"token {i.token} at {i.token_address} is not a recognised stablecoin contract"
        elif i.payer in own:
            i.verdict, i.category = "EXCLUDED", "own-transfer"
            i.basis = "sent from another of the earner's own addresses"
        elif twin is not None and amount <= DUST:
            i.verdict, i.category = "EXCLUDED", "poisoning-bait"
            i.basis = f"dust from {i.payer}, which imitates {twin} (same first and last characters)"
        elif amount <= DUST:
            i.verdict, i.category = "EXCLUDED", "dust"
            i.basis = f"{i.amount} {i.token}: at or below one unit, not evidence of anything"
        elif i.payer_flagged_scam:
            i.verdict, i.category = "EXCLUDED", "flagged-scam"
            i.basis = "the explorer flags the sender as a scam"
        elif twin is not None:
            # Real money from a look-alike is not bait, but nobody should
            # read this line without seeing that it is a look-alike.
            i.verdict, i.category = "ASK", "lookalike-payer"
            i.basis = f"{i.payer} imitates {twin}; confirm this payer is genuine"
        elif stated is not None:
            cat = stated.get("category", "other")
            i.verdict = "INCLUDED"
            i.category = cat if cat in CATEGORIES else "other"
            i.basis = f"earner states: {stated.get('label', i.category)}"
        elif any(w in names for w in EXCHANGE_WORDS):
            i.verdict, i.category = "INCLUDED", "exchange-withdrawal"
            shown = i.payer_name or ", ".join(i.payer_tags)
            i.basis = f"withdrawal from an exchange account ({shown}), per the explorer's label"
        else:
            i.verdict, i.category = "ASK", "unlabelled"
            who = f"{i.payer_name} ({i.payer})" if i.payer_name else i.payer
            i.basis = f"payer {who} is not labelled; the earner is asked what this payment was for"
    return inflows


def value(inflows: list[Inflow], rates: Rates) -> list[Inflow]:
    for i in inflows:
        if i.verdict == "EXCLUDED":
            continue
        day = datetime.fromisoformat(i.timestamp.replace("Z", "+00:00")).astimezone(timezone.utc).date()
        found = rates.on_or_before(day)
        if found is None:
            continue
        i.rate_date, rate = found[0], found[1]
        i.usd_myr = str(rate)
        i.myr = str((Decimal(i.amount) * rate).quantize(Decimal("0.01")))
    return inflows


# --------------------------------------------------------------- the pack


def summarise(inflows: list[Inflow]) -> dict:
    by: dict[str, dict] = {}
    for i in inflows:
        s = by.setdefault(i.verdict, {}).setdefault(i.category, {"count": 0, "usd": Decimal(0), "myr": Decimal(0)})
        s["count"] += 1
        s["usd"] += Decimal(i.amount) if i.verdict != "EXCLUDED" else Decimal(0)
        s["myr"] += Decimal(i.myr) if i.myr else Decimal(0)
    return {
        v: {c: {"count": x["count"], "usd": str(x["usd"]), "myr": str(x["myr"])} for c, x in cats.items()}
        for v, cats in by.items()
    }


def canonical_sha256(pack: dict) -> str:
    """sha256 over the canonical JSON of everything in the pack but the hash itself."""
    body = {k: v for k, v in pack.items() if k != "sha256"}
    canonical = json.dumps(body, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(canonical).hexdigest()


def verify_hash(pack: dict) -> bool:
    """True if the pack's stated sha256 is the hash of its own contents."""
    return pack.get("sha256") == canonical_sha256(pack)


def build_pack(earner: str, addresses: list[str], chains: list[str], since: date, until: date,
               inflows: list[Inflow], salt: str | None = None) -> dict:
    inflows = sorted(inflows, key=lambda i: (i.timestamp, i.chain, i.tx_hash, i.log_index))
    body = {
        "kind": "tameion.source-of-funds.v0",
        "earner": earner,
        "addresses": [a.lower() for a in addresses],
        "chains": chains,
        "period": {"from": since.isoformat(), "to": until.isoformat()},
        "rate_source": "Bank Negara Malaysia, USD/MYR 17:00 middle rate, business day on or before receipt",
        "stablecoins": {c: sorted(STABLECOINS[c]) for c in chains},
        "summary": summarise(inflows),
        "inflows": [asdict(i) for i in inflows],
        # Random, and inside the hash. The hash may be published (anchor.py);
        # the pack is built from public chain data, so without a salt anyone
        # who could rebuild a pack could confirm that this person had one made.
        "salt": salt if salt is not None else secrets.token_hex(32),
    }
    body["sha256"] = canonical_sha256(body)
    return body
