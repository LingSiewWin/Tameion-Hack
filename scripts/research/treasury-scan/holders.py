import json, sys, time, urllib.request, urllib.parse
USDC="0xaf88d065e77c8cC2239327C5EDb3A432268e5831"
BASE="https://arbitrum.blockscout.com/api/v2"
def get(url):
    for i in range(6):
        try:
            req=urllib.request.Request(url,headers={"User-Agent":"research/0.1","Accept":"application/json"})
            with urllib.request.urlopen(req,timeout=30) as r:
                return json.load(r)
        except Exception as e:
            time.sleep(1.5*(i+1)); last=e
    raise last
pages=int(sys.argv[1]); out=[]; params=None
for p in range(pages):
    url=f"{BASE}/tokens/{USDC}/holders"+("?"+urllib.parse.urlencode(params) if params else "")
    d=get(url)
    for it in d["items"]:
        a=it["address"]
        out.append({"hash":a["hash"],"is_contract":a.get("is_contract"),"name":a.get("name"),
                    "impl":[i.get("name") for i in (a.get("implementations") or [])],
                    "proxy_type":a.get("proxy_type"),"tags":[t.get("name") for t in (a.get("public_tags") or [])],
                    "value":int(it["value"])/1e6})
    params=d.get("next_page_params")
    if not params: break
    time.sleep(0.25)
json.dump(out,open("holders.json","w"))
print(len(out),"holders; min balance",out[-1]["value"])
