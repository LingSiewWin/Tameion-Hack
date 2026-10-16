"""Address poisoning in real treasury histories.
A poisoned address imitates a real payee: same first 4 and last 4 hex chars,
different middle. Attackers plant it with zero-value or dust transfers so that
someone copying 'the payee' from history copies the attacker. Run from data dir."""
import collections
out=collections.defaultdict(list); inn=collections.defaultdict(list)
for l in open('transfers.tsv'):
    f=l.rstrip('\n').split('\t'); out[f[0]].append((f[2].lower(),int(f[3])/1e6,int(f[4])))
for l in open('incoming.tsv'):
    f=l.rstrip('\n').split('\t'); inn[f[0]].append((f[2].lower(),int(f[3])/1e6,int(f[4])))
key=lambda a:(a[2:6],a[-4:])
DUST=1.0
safes=set(out)|set(inn)
hit=[]; zero_out=0; tot_out=0
for s in safes:
    real={}   # genuine payees: received >= $50 at least once
    for to,amt,ts in out.get(s,[]):
        tot_out+=1
        if amt==0: zero_out+=1
        if amt>=50: real.setdefault(key(to),set()).add(to)
    # candidates: addresses seen only with dust/zero value, either direction
    cand=collections.defaultdict(lambda:[0,0])
    for to,amt,ts in out.get(s,[]):
        if amt<=DUST: cand[to][0]+=1
    for fr,amt,ts in inn.get(s,[]):
        if amt<=DUST: cand[fr][1]+=1
    genuine=set().union(*real.values()) if real else set()
    twins=[]
    for a,(zo,di) in cand.items():
        if a in genuine: continue
        k=key(a)
        if k in real:
            twins.append((a,sorted(real[k])[0],zo,di))
    if twins: hit.append((s,twins,len(genuine)))
print('treasuries with history:',len(safes))
print('treasuries with at least one look-alike of a real payee planted in their history:',len(hit),f'({len(hit)/len(safes):.0%})')
print('look-alike addresses in total:',sum(len(t) for _,t,_ in hit))
print('real payees imitated:',sum(len({r for _,r,_,_ in t}) for _,t,_ in hit))
print('planted as a fake OUTGOING transfer (zero value, looks like the treasury paid them):',sum(1 for _,t,_ in hit for x in t if x[2]>0))
print('planted as incoming dust:',sum(1 for _,t,_ in hit for x in t if x[3]>0))
print('zero-value outgoing transfers overall:',zero_out,'of',tot_out)
for s,t,g in sorted(hit,key=lambda x:-len(x[1]))[:6]:
    print(' ',s[:10],'genuine payees',g,'look-alikes',len(t))
    for a,r,zo,di in t[:3]: print('     fake',a[:8]+'…'+a[-6:],' imitates',r[:8]+'…'+r[-6:],' fake-outgoing',zo,' dust-in',di)
# would the shipped detector's input contain them? yes if they appear in outgoing history
inhist=sum(1 for _,t,_ in hit for x in t if x[2]>0)
print('look-alikes that sit in the OUTGOING history an agent reads as "who we pay":',inhist)
