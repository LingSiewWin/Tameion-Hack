"""Reconstruct each Safe's USDC balance backwards from today and report the
lowest point in a trailing window: cash that never moved. Run from the data dir."""
import json,collections,statistics,sys
NOW=int(sys.argv[1]); RATE=0.04   # RATE is an assumption for illustration, not a quoted yield
bal={x['hash']:x['value'] for x in json.load(open('safes.json'))}
def load(fn,ff):
    ev=collections.defaultdict(list)
    for l in open(fn):
        f=l.rstrip('\n').split('\t'); ev[f[0]].append((int(f[4]),int(f[3])/1e6))
    comp={l.split('\t')[0]:int(l.split('\t')[2]) for l in open(ff)}
    return ev,comp
out,co=load('transfers.tsv','fetched.tsv'); inn,ci=load('incoming.tsv','fetched_in.tsv')
for W in (90,180):
    start=NOW-W*86400; rows=[]
    for a,b in bal.items():
        def covered(ev,comp): return comp.get(a,0)==1 or (ev.get(a) and min(t for t,_ in ev[a])<start)
        if not (covered(out,co) and covered(inn,ci)): continue
        evs=sorted([(t,-v) for t,v in out.get(a,[]) if t>=start]+[(t,+v) for t,v in inn.get(a,[]) if t>=start],reverse=True)
        cur=b; mn=b
        for t,d in evs:
            cur-=d; mn=min(mn,cur)
        rows.append((a,b,max(mn,0),sum(v for t,v in out.get(a,[]) if t>=start),len(evs)))
    tb=sum(r[1] for r in rows); tm=sum(r[2] for r in rows)
    print(f'--- trailing {W} days: {len(rows)} of {len(bal)} safes with full coverage')
    print(f'   balance today ${tb/1e6:.1f}M; sum of each safe\'s lowest balance in the window ${tm/1e6:.1f}M = {tm/tb:.0%}')
    print('   never below 50% of today\'s balance:',sum(1 for r in rows if r[2]>=0.5*r[1]),'| never below 90%:',sum(1 for r in rows if r[2]>=0.9*r[1]))
    print('   no movement at all in the window:',sum(1 for r in rows if r[4]==0),'holding $%.1fM'%(sum(r[1] for r in rows if r[4]==0)/1e6))
    fl=[r for r in rows if r[2]>=10000]
    print('   untouched floor >= $10k:',len(fl),f'({len(fl)/len(rows):.0%})')
    print('   median lowest balance $%d; median outflow in window $%d'%(statistics.median(r[2] for r in rows),statistics.median(r[3] for r in rows)))
    print('   forgone on the floor alone at an assumed %.0f%% a year: $%d'%(RATE*100,tm*RATE*W/365))
