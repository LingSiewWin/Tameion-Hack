import os
_HERE=os.path.dirname(os.path.abspath(__file__))
import json,collections,statistics,datetime,re
exec(open(os.path.join(_HERE,'analyse.py')).read().split("safes={};obs=")[0])   # reuse loaders/helpers
T=collections.defaultdict(list)   # safe -> [(ts,to,amt,tx)]
for l in open('transfers.tsv'):
    f=l.rstrip('\n').split('\t'); T[f[0]].append((int(f[4]),f[2].lower(),int(f[3])/1e6,f[1]))
def cv(v):
    if len(v)<2: return 0
    m=statistics.mean(v); return statistics.stdev(v)/m if m else 0
ok=lambda a: payee_class(a) in('eoa','safe','contract-unnamed')
n_out=[len(v) for v in T.values()]
print('outgoing transfers per safe: median',statistics.median(n_out),'p75',statistics.quantiles(n_out,n=4)[2])
print('history span days per safe: median',round(statistics.median((s[1]-s[0])/86400 for s in span.values())))
# payee-level repeat structure
rep3=rep5=0; safes3=set(); safes5=set(); gapcv=[]; rows=[]
for s,v in T.items():
    by=collections.defaultdict(list)
    for ts,to,amt,tx in v:
        if ok(to) and amt>=50: by[to].append((ts,amt))
    for to,p in by.items():
        p.sort(); 
        # collapse same-day duplicates
        gaps=[(p[i+1][0]-p[i][0])/86400 for i in range(len(p)-1) if p[i+1][0]>p[i][0]]
        if len(gaps)+1>=3:
            rep3+=1; safes3.add(s)
            med=statistics.median(gaps); c=cv(gaps)
            rows.append((s,to,len(gaps)+1,med,c,statistics.median(a for _,a in p),cv([a for _,a in p])))
            if len(gaps)+1>=5: rep5+=1; safes5.add(s)
print('safes with a non-protocol payee paid >=3 times:',len(safes3),' >=5 times:',len(safes5),' (of',len(T),'with any outgoing)')
print('payee relationships with >=3 payments:',rep3,' >=5:',rep5)
bill=[r for r in rows if 6<=r[3]<=100]
print('  of those with median gap 6-100d:',len(bill))
b=collections.Counter()
for r in bill:
    c=r[4]; b['cv<=0.35' if c<=0.35 else 'cv 0.35-0.7' if c<=0.7 else 'cv>0.7']+=1
print('  timing regularity among them:',dict(b))
reg=[r for r in bill if r[4]<=0.35]
print('  regular ones by sample count:',dict(sorted(collections.Counter(min(r[2],8) for r in reg).items())),'(8 = 8+)')
print('  safes having >=1 regular 6-100d payee at n>=3:',len({r[0] for r in reg}),' n>=4:',len({r[0] for r in reg if r[2]>=4}),' n>=5:',len({r[0] for r in reg if r[2]>=5}))
cad=collections.Counter()
for r in reg:
    d=r[3]; cad['weekly' if d<=8 else 'biweekly' if 12<=d<=17 else 'monthly' if 26<=d<=33 else 'quarterly' if 80<=d<=100 else 'other']+=1
print('  cadence of regular ones:',dict(cad))
# run-level: batches = same tx hash with >=3 non-protocol payees
runsafes=0; mr=[]
for s,v in T.items():
    bytx=collections.defaultdict(list)
    for ts,to,amt,tx in v:
        if ok(to): bytx[tx].append((ts,to,amt))
    runs=sorted((x[0][0],len(x),sum(a for _,_,a in x)) for x in bytx.values() if len(x)>=3)
    if len(runs)>=3:
        gaps=[(runs[i+1][0]-runs[i][0])/86400 for i in range(len(runs)-1)]
        gaps=[g for g in gaps if g>0.5]
        if len(gaps)>=2:
            runsafes+=1; mr.append((s,len(runs),statistics.median(gaps),cv(gaps),statistics.median(r[1] for r in runs),statistics.median(r[2] for r in runs)))
print('safes with >=3 batch payouts (one tx paying >=3 non-protocol payees):',runsafes)
print('  of which batch cadence regular (cv<=0.35, median gap 6-100d):',sum(1 for r in mr if r[3]<=0.35 and 6<=r[2]<=100))
for r in sorted(mr,key=lambda r:r[3])[:12]:
    print('   ',r[0][:10],'runs',r[1],'every %.1fd cv %.2f'%(r[2],r[3]),'payees/run',r[4],'$/run',round(r[5]),'bal',round(bal[r[0]]))
# what dominates outgoing? destination class by count and value
dc=collections.Counter(); dv=collections.Counter()
for s,v in T.items():
    for ts,to,amt,tx in v:
        k=payee_class(to); k='contract-other' if k.startswith('contract-other') else k
        dc[k]+=1; dv[k]+=amt
print('outgoing by destination class (count):',dict(dc))
print('outgoing by destination class ($M):',{k:round(x/1e6,1) for k,x in dv.items()})
