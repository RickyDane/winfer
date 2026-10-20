"""Medians per task and arm for codex_run.py results. 'units' = uncached input + 0.1 x cached + 8 x output tokens
(OpenAI's price ratios), a cost proxy since Codex on a ChatGPT plan reports no dollar cost."""
import json, glob, os, statistics as st, collections
B = os.environ.get('CGBENCH_DIR', '/tmp/cgbench') + '/codex'
def parse(p):
    u = w = None; tools = collections.Counter(); err = False
    for l in open(p):
        try: o = json.loads(l)
        except Exception: continue
        t = o.get('type')
        if t == 'turn.completed': u = o['usage']
        elif t == 'bench.wall': w = o['seconds']
        elif t == 'item.completed' and o['item']['type'] in ('command_execution', 'mcp_tool_call', 'file_change', 'web_search'):
            tools['explore' if o['item'].get('tool') == 'codegraph_explore' else o['item']['type']] += 1
        elif t in ('turn.failed', 'error'): err = True
    if not u: return None
    unc = u['input_tokens'] - u['cached_input_tokens']
    return dict(units=unc + 0.1 * u['cached_input_tokens'] + 8 * u['output_tokens'], inp=u['input_tokens'], out=u['output_tokens'],
                wall=w or 0, tools=sum(tools.values()), explore=tools['explore'], err=err)
rows = collections.defaultdict(list)
for p in sorted(glob.glob(f'{B}/*/*.jsonl')):
    r = parse(p)
    if r: rows[(p.split('/')[-2], os.path.basename(p).rsplit('-', 1)[0])].append(r)
m = lambda rs, k: st.median(r[k] for r in rs)
print(f"{'task':4} {'arm':8} n {'units':>8} {'input':>8} {'out':>6} {'wall s':>6} {'tools':>5} {'expl':>4}")
for (t, a), rs in sorted(rows.items()):
    print(f"{t:4} {a:8} {len(rs)} {m(rs,'units'):8.0f} {m(rs,'inp'):8.0f} {m(rs,'out'):6.0f} {m(rs,'wall'):6.0f} {m(rs,'tools'):5.1f} {m(rs,'explore'):4.1f}" + (' ERR' if any(r['err'] for r in rs) else ''))
tot = collections.defaultdict(float)
for (t, a), rs in rows.items(): tot[a] += m(rs, 'units')
print({a: round(v) for a, v in tot.items()})
