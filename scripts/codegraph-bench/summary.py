import json, glob, os, statistics as st, collections, sys
B=os.environ.get('CGBENCH_DIR','/tmp/cgbench')+'/results'
def parse(p):
    tools=collections.Counter(); seen=set(); res=None; exp_chars=0; reads=0
    for l in open(p):
        try:o=json.loads(l)
        except:continue
        if o.get('type')=='assistant':
            for c in o['message'].get('content',[]):
                if c.get('type')=='tool_use' and c['id'] not in seen:
                    seen.add(c['id']); n=c['name'].split('__')[-1]; tools[n]+=1
        if o.get('type')=='user' and isinstance(o.get('message',{}).get('content'),list):
            for c in o['message']['content']:
                if c.get('type')=='tool_result':
                    t=c['content'] if isinstance(c['content'],str) else ''.join(x.get('text','') for x in c['content'] if isinstance(x,dict))
                    if 'Source Code' in t or 'Exploration' in t: exp_chars+=len(t)
        if o.get('type')=='result': res=o
    if not res: return None
    u=res.get('usage',{})
    return dict(cost=res.get('total_cost_usd',0), dur=res.get('duration_ms',0)/1000, turns=res.get('num_turns',0), tools=sum(tools.values()),
                explore=tools['codegraph_explore'], read=tools['Read'], grep=tools['Grep']+tools['Glob'], bash=tools['Bash'], agent=tools['Agent'],
                tok=u.get('input_tokens',0)+u.get('output_tokens',0)+u.get('cache_read_input_tokens',0)+u.get('cache_creation_input_tokens',0),
                out=u.get('output_tokens',0), exp_chars=exp_chars, err=res.get('is_error',False), subtype=res.get('subtype'))
rows=collections.defaultdict(list)
for p in sorted(glob.glob(f'{B}/*/*.jsonl')):
    task=p.split('/')[-2]; arm=os.path.basename(p).rsplit('-',1)[0]; r=parse(p)
    if r: rows[(task,arm)].append(r)
med=lambda xs: st.median(xs) if xs else 0
print(f"{'task':4} {'arm':7} n  {'cost$':>6} {'time s':>6} {'tools':>5} {'expl':>4} {'read':>4} {'grep':>4} {'bash':>4} {'agent':>5} {'tokens':>8} {'out':>6}")
for (task,arm),rs in sorted(rows.items()):
    m=lambda k: med([r[k] for r in rs])
    print(f"{task:4} {arm:7} {len(rs)}  {m('cost'):6.2f} {m('dur'):6.0f} {m('tools'):5.1f} {m('explore'):4.1f} {m('read'):4.1f} {m('grep'):4.1f} {m('bash'):4.1f} {m('agent'):5.1f} {m('tok'):8.0f} {m('out'):6.0f}" + ("  ERR" if any(r['err'] for r in rs) else ""))
if '-v' in sys.argv:
    for (task,arm),rs in sorted(rows.items()):
        print(task,arm,[ (round(r['cost'],2), r['tools'], r['explore']) for r in rs])
