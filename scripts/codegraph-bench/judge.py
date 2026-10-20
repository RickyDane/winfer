import json, glob, os, random, subprocess, sys, re
B=os.environ.get('CGBENCH_DIR','/tmp/cgbench'); sys.path.insert(0,os.path.dirname(os.path.abspath(__file__)))
from run import TASKS
random.seed(7)
def answer(p):
    for l in open(p):
        if '"type":"result"' in l: return json.loads(l).get('result','')
    return ''
for task in sys.argv[1:]:
    repo, edit, q = TASKS[task]
    ARMS_J=tuple(os.environ.get('JUDGE_ARMS','plain,cg,cg_scouth').split(','))
    runs=sorted(p for p in glob.glob(f'{B}/results/{task}/*.jsonl') if os.path.basename(p).rsplit('-',1)[0] in ARMS_J); random.shuffle(runs)
    labels={}; parts=[]
    for i,p in enumerate(runs):
        lab=f'S{i+1}'; labels[lab]=os.path.basename(p)[:-6]
        body=answer(p)
        if edit:
            d=open(p[:-6]+'.diff').read()
            body=f"Final message:\n{body}\n\nDiff of its changes (git diff + untracked file list):\n{d[:30000]}"
        parts.append(f"===== {lab} =====\n{body}\n")
    prompt=(f"You are grading {len(runs)} independent submissions for this task in the repository at {repo}:\n\nTASK: {q}\n\n"
        "Verify claims against the actual code (use Read/Grep/Bash, read-only; do not modify files). For each submission give: "
        "correctness 0-10 (are the stated facts, hops and file:line references right; for edit tasks: would the change work and follow the repo's patterns), "
        "completeness 0-10 (does it cover what the task asks, missing key hops/layers or parts of the change), and one short note. "
        "Grade each on its own merits; the submissions are anonymous and in random order. "
        "End your reply with ONLY a JSON object on the last line: {\"S1\": {\"correctness\": n, \"completeness\": n, \"note\": \"...\"}, ...}\n\n" + "\n".join(parts))
    out=subprocess.run(['claude','-p',prompt,'--model','opus','--output-format','json','--tools','Read,Grep,Glob,Bash','--dangerously-skip-permissions',
                        '--strict-mcp-config','--setting-sources','project,local','--disable-slash-commands','--max-budget-usd','10'],
                       cwd=repo,capture_output=True,text=True,stdin=subprocess.DEVNULL,timeout=3000).stdout
    try:
        ev=json.loads(out); text=ev['result'] if isinstance(ev,dict) else [e for e in ev if e.get('type')=='result'][-1]['result']
    except Exception: text=out
    open(f'{B}/results/{task}/judge'+os.environ.get('JUDGE_TAG','2')+'.raw.txt','w').write(text)
    scores={}
    i=text.rfind('{"S')
    while i>=0 and not scores:
        try: scores=json.JSONDecoder().raw_decode(text[i:])[0]
        except Exception: i=text.rfind('{"S',0,i)
    res={labels[k]:v for k,v in scores.items() if k in labels}
    json.dump({'task':task,'scores':res,'raw':text}, open(f'{B}/results/{task}/judge'+os.environ.get('JUDGE_TAG','2')+'.json','w'), indent=1)
    print(task, json.dumps({k:(v['correctness'],v['completeness']) for k,v in sorted(res.items())}), flush=True)
