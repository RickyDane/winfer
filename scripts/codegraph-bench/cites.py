import json, glob, os, re, subprocess, collections, statistics as st
B=os.environ.get('CGBENCH_DIR','/tmp/cgbench')+'/results'
REPO={'V':'/Users/rickyperlick/Coding/bench/vscode','W':'/Users/rickyperlick/Coding/winfer'}
files={}
def ls(repo):
    if repo not in files:
        fs=subprocess.run(['git','ls-files'],cwd=repo,capture_output=True,text=True).stdout.split('\n')
        by=collections.defaultdict(list)
        for f in fs:
            if f: by[os.path.basename(f)].append(f)
        files[repo]=(set(fs),by)
    return files[repo]
nlines={}
def length(repo,f):
    k=(repo,f)
    if k not in nlines:
        try: nlines[k]=sum(1 for _ in open(os.path.join(repo,f),errors='ignore'))
        except: nlines[k]=0
    return nlines[k]
CITE=re.compile(r'([\w./@-]+\.(?:ts|tsx|js|mjs|rs|json|css|py))[`]?(?:[:#]L?(\d+)(?:[-–](\d+))?)')
def check(text, repo):
    allf,by=ls(repo); ok=bad=0; uniq=set()
    for m in CITE.finditer(text):
        path,line=m.group(1).lstrip('./'),int(m.group(2))
        cands=[path] if path in allf else [f for f in by.get(os.path.basename(path),[]) if f.endswith(path)]
        good=any(0<line<=length(repo,c) for c in cands)
        ok+=good; bad+=not good; uniq.add((cands[0] if cands else path, line))
    return ok,bad,len(uniq)
rows=collections.defaultdict(list)
for p in sorted(glob.glob(f'{B}/*/*.jsonl')):
    task=p.split('/')[-2]; arm=os.path.basename(p).rsplit('-',1)[0]
    res=None
    for l in open(p):
        if '"type":"result"' in l: res=json.loads(l)
    if not res: continue
    ok,bad,u=check(res.get('result',''), REPO[task[0]])
    rows[(task,arm)].append((ok,bad,u,len(res.get('result',''))))
print('task arm     cites(valid/invalid/unique)  answer chars')
for k,v in sorted(rows.items()):
    print(f"{k[0]:4} {k[1]:7}", '  '.join(f"{a}/{b}/{c}" for a,b,c,_ in v), '   ', [d for *_,d in v])
