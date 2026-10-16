import json,collections,statistics,datetime,re
NOW=int(datetime.datetime(2026,9,30,tzinfo=datetime.timezone.utc).timestamp())
bal={x['hash']:x['value'] for x in json.load(open('safes.json'))}
fetched={l.split('\t')[0]:(int(l.split('\t')[1]),int(l.split('\t')[2])) for l in open('fetched.tsv')}
# recipient metadata
meta={}
span={}
for l in open('transfers.tsv'):
    f=l.rstrip('\n').split('\t')
    meta[f[2].lower()]=(f[5],f[6])
    s=span.setdefault(f[0],[int(f[4]),int(f[4])]); s[0]=min(s[0],int(f[4])); s[1]=max(s[1],int(f[4]))
PROTO=re.compile(r'bridge|router|pool|vault|aave|uniswap|messenger|spoke|exchange|swap|market|aggregat|permit|1inch|proxy(?!.*safe)|diamond|atoken|comet|morpho|gmx|across|stargate|oft|settlement|cow|paraswap|odos|relay|deposit|wrapper|token',re.I)
def payee_class(a):
    c,n=meta.get(a,('0',''))
    if c=='0': return 'eoa'
    if 'safe' in n.lower(): return 'safe'
    if n=='' : return 'contract-unnamed'
    if PROTO.search(n): return 'protocol'
    return 'contract-other:'+n
safes={};obs=collections.defaultdict(list)
for l in open('probe.out'):
    f=l.rstrip('\n').split('\t')
    if f[0]=='SAFE': safes[f[1]]=dict(n=int(f[2]),payees=int(f[3]),nobs=int(f[4]))
    else:
        obs[f[1]].append(dict(cp=f[2],kind=f[3],amt=int(f[4])/1e6,days=float(f[5]),n=int(f[6]),conf=float(f[7]),icv=float(f[8]),acv=float(f[9]),due=int(f[10]),cls=payee_class(f[2])))
total=len(fetched)
none=[a for a,(n,c) in fetched.items() if n==0]
print('safes sampled',total,'| with zero outgoing native-USDC transfers',len(none),'| truncated at 500:',sum(1 for a,(n,c) in fetched.items() if not c))
def billlike(o): return 6<=o['days']<=100 and o['conf']>=0.75 and o['cls'] in('eoa','safe','contract-unnamed') and o['amt']>=50
def live(o): return o['due']>=NOW-int(1.5*o['days']*86400)   # last paid within ~2.5 cycles
any_ob=[a for a in fetched if obs.get(a)]
bl={a:[o for o in obs[a] if billlike(o)] for a in fetched}
bl={a:v for a,v in bl.items() if v}
lv={a:[o for o in v if live(o)] for a,v in bl.items()}
lv={a:v for a,v in lv.items() if v}
active=[a for a,(n,c) in fetched.items() if n>=3]
print('safes with >=3 outgoing transfers (detector can run at all):',len(active))
print('safes with any detected pattern:',len(any_ob))
print('safes with >=1 bill-like obligation (6-100d, conf>=0.75, payee not a protocol, >=$50):',len(bl))
print('  of which still live (last payment within 2.5 cycles):',len(lv))
allb=[o for v in bl.values() for o in v]; alll=[o for v in lv.values() for o in v]
print('bill-like obligations total',len(allb),'live',len(alll))
print('per-safe count of bill-like: median',statistics.median(len(v) for v in bl.values()),'max',max(len(v) for v in bl.values()))
print('amount median $',round(statistics.median(o['amt'] for o in allb)),' p25',round(statistics.quantiles([o['amt'] for o in allb],n=4)[0]),' p75',round(statistics.quantiles([o['amt'] for o in allb],n=4)[2]))
iv=collections.Counter()
for o in allb:
    d=o['days']; iv['weekly(6-8d)' if d<=8 else 'biweekly(13-16d)' if 13<=d<=16 else 'monthly(27-32d)' if 27<=d<=32 else 'quarterly(85-95d)' if 85<=d<=95 else 'other']+=1
print('cadence',dict(iv))
print('kind',collections.Counter(o['kind'] for o in allb))
print('payee class',collections.Counter(o['cls'] for o in allb))
print('monthly-equivalent $ across bill-like obligations:',round(sum(o['amt']*30/o['days'] for o in allb)))
print('USDC balance held by safes with bill-like obligations: $',round(sum(bal[a] for a in bl)),' of $',round(sum(bal[a] for a in fetched)))
# rejected classes
rej=collections.Counter()
for a in fetched:
    for o in obs.get(a,[]):
        if not billlike(o):
            rej['protocol payee' if o['cls'] not in('eoa','safe','contract-unnamed') else 'too frequent (<6d)' if o['days']<6 else 'too slow (>100d)' if o['days']>100 else 'confidence<0.75' if o['conf']<0.75 else 'tiny']+=1
print('patterns detected but not bill-like:',dict(rej))
print()
print('TOP examples (safe, balance, obligations)')
for a,v in sorted(bl.items(),key=lambda kv:-len(kv[1]))[:8]:
    print(a,'bal $%d'%bal[a],'outgoing',fetched[a][0],'payees',safes[a]['payees'],'bills',len(v))
    for o in sorted(v,key=lambda o:-o['amt'])[:4]:
        print('    %s  $%.2f every %.1fd  n=%d conf=%.2f %s %s %s'%(o['cp'][:10],o['amt'],o['days'],o['n'],o['conf'],o['kind'],o['cls'],'LIVE' if live(o) else 'lapsed'))
json.dump({'bl':bl,'fetched':fetched},open('result.json','w'))
