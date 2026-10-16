import os
_HERE=os.path.dirname(os.path.abspath(__file__))
import collections,re,statistics
exec(open(os.path.join(_HERE,'analyse.py')).read().split("safes={};obs=")[0])
T=collections.defaultdict(list)
for l in open('transfers.tsv'):
    f=l.rstrip('\n').split('\t'); T[(f[0],f[2].lower())].append((int(f[4]),int(f[3])))
trunc={a for a,(n,c) in fetched.items() if not c}
rows=[]
for l in open('walk.out'):
    _,safe,cut,cp,amt,iv,due,conf,n,kind,acv,icv=l.rstrip('\n').split('\t'); icv=float(icv)
    cut=int(cut);amt=int(amt);iv=int(iv);due=int(due);conf=float(conf);n=int(n)
    if payee_class(cp) not in('eoa','safe','contract-unnamed'): continue
    if not (6*86400<=iv<=100*86400) or amt<50e6: continue
    tol=max(3*86400,int(0.25*iv))
    near=[(abs(ts-due),a) for ts,a in T[(safe,cp)] if ts>=cut and abs(ts-due)<=tol]
    paid=bool(near)
    amt_ok=paid and abs(min(near)[1]-amt)<=0.10*amt
    exact=paid and min(near)[1]==amt
    ever=any(ts>=cut for ts,a in T[(safe,cp)])
    rows.append((conf,n,paid,amt_ok,ever,safe,iv/86400,min(near)[0]/86400 if near else None,amt,kind,icv,exact))
print('predictions scored:',len(rows),'across',len({r[5] for r in rows}),'treasuries')
def rep(name,rs):
    if not rs: print(name,'none'); return
    p=sum(r[2] for r in rs)/len(rs); a=sum(r[3] for r in rs)/len(rs); e=sum(r[4] for r in rs)/len(rs)
    err=[r[7] for r in rs if r[7] is not None]
    print(f'{name:28s} n={len(rs):4d}  paid on schedule {p:5.1%}  and within 10% of amount {a:5.1%}  payee paid again at all {e:5.1%}  median date error {statistics.median(err):.1f}d' if err else f'{name} n={len(rs)} paid {p:.1%}')
rep('all',rows)
rep('confidence >= 0.75 (floor)',[r for r in rows if r[0]>=0.75])
rep('confidence 0.60-0.75',[r for r in rows if 0.60<=r[0]<0.75])
rep('confidence 0.45-0.60',[r for r in rows if 0.45<=r[0]<0.60])
rep('confidence < 0.45',[r for r in rows if r[0]<0.45])
print()
rep('samples 3',[r for r in rows if r[1]==3]); rep('samples 4-5',[r for r in rows if 4<=r[1]<=5]); rep('samples 6+',[r for r in rows if r[1]>=6])
print()
tot=sum(r[8] for r in rows)/1e6; hit=sum(r[8] for r in rows if r[3])/1e6; miss=sum(r[8] for r in rows if not r[2])/1e6
print(f'$ predicted {tot:,.0f}; $ where payment arrived on schedule within 10%: {hit:,.0f}; $ predicted but no payment came: {miss:,.0f}')

print()
def rep2(name,rs):
    if not rs: print(name,'none'); return
    f=lambda i: sum(r[i] for r in rs)/len(rs)
    print(f"{name:44s} n={len(rs):4d} safes={len({r[5] for r in rs}):2d}  on schedule {f(2):5.1%}  +within 10% {f(3):5.1%}  +exact amount {f(11):5.1%}")
rep2('fixed amount',[r for r in rows if r[9]=='subscription'])
rep2('variable amount',[r for r in rows if r[9]!='subscription'])
rep2('fixed amount, 5+ samples, timing cv<=0.15',[r for r in rows if r[9]=='subscription' and r[1]>=5 and r[10]<=0.15])
rep2('timing cv<=0.15, 5+ samples',[r for r in rows if r[1]>=5 and r[10]<=0.15])
rep2('timing cv 0.15-0.35, 5+ samples',[r for r in rows if r[1]>=5 and r[10]>0.15])
c=collections.Counter(r[5] for r in rows)
print('predictions from the four busiest treasuries:',sum(v for _,v in c.most_common(4)),'of',len(rows))
