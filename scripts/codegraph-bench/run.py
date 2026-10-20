import json, os, subprocess, sys, time, shutil, concurrent.futures as cf
B = os.environ.get('CGBENCH_DIR', '/tmp/cgbench')
import re
S = os.path.dirname(os.path.abspath(__file__))  # this folder: arms.py, block.sh
ROOT = os.path.dirname(os.path.dirname(S))     # the winfer checkout the prompts are read from
def _const(path, name):
    return re.search(r'(?:const|pub const) ' + name + r'(?:: &str)? =\s*"(.*?)";', open(os.path.join(ROOT, path)).read(), re.S).group(1).replace('\\"', '"')
LEAN = _const('src-tauri/src/harness/backend/mod.rs', 'LEAN_PROMPT')
LEANCG = _const('src-tauri/src/harness/backend/mod.rs', 'LEAN_CODEGRAPH_PROMPT')
TOOLS = _const('src-tauri/src/harness/backend/claude.rs', 'LEAN_TOOLS')
DEFAULT = re.search(r'DEFAULT_SYSTEM_PROMPT =\s*"(.*?)";', open(os.path.join(ROOT, 'src/stores/workspaces.ts')).read(), re.S).group(1)
VS, WF = '/Users/rickyperlick/Coding/bench/vscode', '/Users/rickyperlick/Coding/winfer'
CG = '/Users/rickyperlick/.local/bin/codegraph'
TASKS = {
 'V1': (VS, False, "How does a keystroke in the editor end up in ITextModel.applyEdits? Trace the path with file:line for each hop."),
 'V2': (VS, False, "How does the extension host communicate with the main process?"),
 'V3': (VS, False, "Explain end to end how a CompletionItemProvider that an extension registers with vscode.languages.registerCompletionItemProvider gets its items shown in the editor's suggest widget. Cite file:line for each layer."),
 'V4': (VS, False, "Give an architecture overview of VS Code's configuration (settings) system: where default, user, workspace and folder settings come from, how they're merged, and how a change reaches listeners like onDidChangeConfiguration. Cite file:line."),
 'V5': (VS, True, "Add a new editor action \"Sort Lines Ascending (Case Insensitive)\" next to the existing sort lines actions, with the matching command logic and unit tests next to the existing sort lines tests. Follow the existing patterns. Don't run builds or tests."),
 'W1': (WF, False, "How does a spoken voice command get from the microphone to an action in a pane? Trace the path through the frontend and the Rust backend with file:line."),
 'W2': (WF, True, "Add a kb_files agent tool to the Knowledge Base plugin (plugins/knowledge-base/index.js) that lists the files in the project's knowledge graph, optionally under a folder: with codegraph through `codegraph files`, with graphify from the loaded graph's nodes. Register it like the other kb tools, add a test in scripts/knowledge-base.test.mjs for any pure helper you add, and run that test file with npx vitest."),
}
ARMS = {
 'plain': {}, 'cg': {'mcp': True}, 'cg_old': {'mcp': True, 'prompt': LEAN},
 'full_plain': {'full': True}, 'full_cg': {'mcp': True, 'full': True},
}
try:
    exec(open(f'{S}/arms.py').read())
except FileNotFoundError: pass
def env_line(cwd):
    br = subprocess.run(['git','rev-parse','--abbrev-ref','HEAD'],cwd=cwd,capture_output=True,text=True).stdout.strip()
    return f"Environment: cwd {cwd} · macos · 2026-10-20 · git branch {br}"
def prepare(task, tag):
    repo, edit, _ = TASKS[task]
    if not edit: return repo
    wt = f'{B}/wt/{task}-{tag}'
    if os.path.exists(wt):
        subprocess.run(['git','worktree','remove','--force',wt],cwd=repo,capture_output=True)
    subprocess.run(['git','worktree','add','-q','--detach',wt,'HEAD'],cwd=repo,check=True,capture_output=True)
    os.makedirs(f'{wt}/.codegraph')
    for f in ['codegraph.db','codegraph.db-wal','codegraph.db-shm','.gitignore']:
        if os.path.exists(f'{repo}/.codegraph/{f}'): subprocess.run(['cp','-c',f'{repo}/.codegraph/{f}',f'{wt}/.codegraph/{f}'],check=True)
    if os.path.exists(f'{repo}/node_modules'): os.symlink(f'{repo}/node_modules', f'{wt}/node_modules')
    return wt
def run(task, arm, i):
    tag = f'{arm}-{i}'; out = f'{B}/results/{task}/{tag}.jsonl'
    if os.path.exists(out) and os.path.getsize(out) > 0 and '"type":"result"' in open(out).read(): return out
    os.makedirs(os.path.dirname(out), exist_ok=True)
    cwd = prepare(task, tag)
    a = ARMS.get(arm) or ARMS[arm.split('.')[0]]
    prompt = a.get('prompt', LEANCG if a.get('mcp') else LEAN)
    lean = [] if a.get('full') else ['--tools',a.get('tools',TOOLS),'--disable-slash-commands','--system-prompt', f'{prompt}\n\n{env_line(cwd)}','--append-system-prompt',DEFAULT]
    hooks = {'PreToolUse':[{'matcher':'Bash','hooks':[{'type':'command','command':f'{S}/block.sh'}]}]}
    hooks.update(a.get('hooks', {}))
    args = ['claude','-p','--output-format','stream-json','--verbose','--model','opus','--dangerously-skip-permissions',
            '--strict-mcp-config','--setting-sources','project,local', *lean, '--settings', json.dumps({'hooks':hooks}), '--max-budget-usd','8', *a.get('extra',[])]
    if a.get('mcp'):
        env = {'CODEGRAPH_TELEMETRY':'0','CODEGRAPH_NO_UPDATE_CHECK':'1', **a.get('env',{})}
        args += ['--mcp-config', json.dumps({'mcpServers':{'codegraph':{'command':CG,'args':['serve','--mcp','--path',cwd],'env':env,'alwaysLoad':True}}})]
    run_env = {**os.environ, **a.get('run_env',{})}
    args.insert(2, TASKS[task][2])  # --mcp-config takes several values, so the prompt goes first
    t = time.time()
    with open(out,'w') as f:
        try: subprocess.run(args, cwd=cwd, env=run_env, stdout=f, stderr=subprocess.DEVNULL, stdin=subprocess.DEVNULL, timeout=1500)
        except subprocess.TimeoutExpired: pass
    if TASKS[task][1]:
        diff = subprocess.run(['git','diff','--stat'],cwd=cwd,capture_output=True,text=True).stdout + subprocess.run(['git','status','--porcelain'],cwd=cwd,capture_output=True,text=True).stdout
        full = subprocess.run(['git','diff'],cwd=cwd,capture_output=True,text=True).stdout
        unt = subprocess.run(['git','ls-files','--others','--exclude-standard'],cwd=cwd,capture_output=True,text=True).stdout
        open(out.replace('.jsonl','.diff'),'w').write(diff+'\n'+full+'\nUNTRACKED:\n'+unt)
    print(f'{task} {tag} done in {time.time()-t:.0f}s', flush=True)
    return out
if __name__ == '__main__':
    # usage: python3 -I run.py <parallel> TASK:ARM:N ...  (results in $CGBENCH_DIR/results)
    jobs = [tuple(j.split(':')) for j in sys.argv[2:]]
    with cf.ThreadPoolExecutor(int(sys.argv[1])) as ex:
        for f in cf.as_completed([ex.submit(run, t, a, int(i)) for t, a, i in jobs]): f.result()
