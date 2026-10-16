import json,urllib.request,time,sys,collections
now=int(time.time()*1000); start=now-730*86400*1000
def hist(coin):
    out=[];t=start
    while t<now:
        time.sleep(1.5)
        req=urllib.request.Request("https://api.hyperliquid.xyz/info",data=json.dumps({"type":"fundingHistory","coin":coin,"startTime":t}).encode(),headers={"Content-Type":"application/json"})
        rows=json.load(urllib.request.urlopen(req,timeout=30))
        if not rows: break
        out+=rows; t=rows[-1]["time"]+1
        if len(rows)<500: break
    return out
for c in sys.argv[1:]:
    m=collections.OrderedDict(); r=hist(c)
    for x in r:
        k=time.strftime("%Y-%m",time.gmtime(x["time"]/1000)); m[k]=m.get(k,0)+float(x["fundingRate"])
    tot=sum(float(x["fundingRate"]) for x in r); days=(r[-1]["time"]-r[0]["time"])/86400000
    print(f"{c}: {r[0]['time']//1000} .. {r[-1]['time']//1000} days={days:.0f} total={tot*100:.2f}% per-year={tot*100*365/days:.2f}% negative months={sum(1 for v in m.values() if v<0)}/{len(m)}")
    print("  monthly short-side %:"," ".join(f"{k[2:]}:{v*100:+.2f}" for k,v in m.items()))
