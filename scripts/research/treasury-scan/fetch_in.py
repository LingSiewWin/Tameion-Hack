import json, sys, time, urllib.request, urllib.parse, threading, datetime
from concurrent.futures import ThreadPoolExecutor
USDC="0xaf88d065e77c8cC2239327C5EDb3A432268e5831"
BASE="https://arbitrum.blockscout.com/api/v2"
MAXP=int(sys.argv[2]) if len(sys.argv)>2 else 10
lock=threading.Lock()
def get(url):
    last=None
    for i in range(6):
        try:
            req=urllib.request.Request(url,headers={"User-Agent":"research/0.1","Accept":"application/json"})
            with urllib.request.urlopen(req,timeout=40) as r:
                return json.load(r)
        except Exception as e:
            last=e; time.sleep(2*(i+1))
    raise last
def one(addr):
    rows=[]; params=None; pages=0; complete=False
    try:
        while pages<MAXP:
            q={"type":"ERC-20","filter":"to","token":USDC}
            if params: q.update(params)
            d=get(f"{BASE}/addresses/{addr}/token-transfers?"+urllib.parse.urlencode(q))
            for it in d["items"]:
                if it["to"]["hash"].lower()!=addr.lower(): continue
                ts=int(datetime.datetime.fromisoformat(it["timestamp"].replace("Z","+00:00")).timestamp())
                to=it["from"]
                rows.append((addr,it["transaction_hash"],to["hash"],it["total"]["value"],ts,
                             "1" if to.get("is_contract") else "0",(to.get("name") or "")))
            pages+=1
            params=d.get("next_page_params")
            if not params: complete=True; break
    except Exception as e:
        sys.stderr.write(f"ERR {addr} {e}\n")
    with lock:
        with open("incoming.tsv","a") as f:
            for r in rows: f.write("\t".join(map(str,r))+"\n")
        with open("fetched_in.tsv","a") as f:
            f.write(f"{addr}\t{len(rows)}\t{int(complete)}\n")
safes=[x["hash"] for x in json.load(open(sys.argv[1]))]
try: done={l.split("\t")[0] for l in open("fetched_in.tsv")}
except FileNotFoundError: done=set()
todo=[s for s in safes if s not in done]
print(len(todo),"to fetch",flush=True)
with ThreadPoolExecutor(4) as ex: list(ex.map(one,todo))
print("done",flush=True)
