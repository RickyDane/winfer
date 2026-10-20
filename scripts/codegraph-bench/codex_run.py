"""Codex version of run.py: a stock Codex pane as winfer starts it (winfer's default prompt as
developer_instructions), with and without codegraph's MCP server. usage: codex_run.py <parallel> TASK:ARM:N ..."""
import json, os, subprocess, sys, time, concurrent.futures as cf
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from run import TASKS, DEFAULT, CG, B, prepare
HINT = ("This project has a codegraph index (MCP tool codegraph_explore). To find or understand code, call codegraph_explore first, "
        "naming the 2-4 symbols or files at the heart of the question (for a flow, its two ends and the hop you're unsure of), not every hop. "
        "Treat the source it returns as already read and don't re-check it with rg. Use rg and sed only for what it didn't cover: hops dispatched "
        "by a string (command ids, events, IPC channels), configs, docs, and exact lists of every use of a name.")
ARMS = {'plain': (DEFAULT, False), 'cg': (DEFAULT, True), 'cg_hint': (DEFAULT + '\n\n' + HINT, True)}
toml = json.dumps  # a JSON string is a valid TOML string
PATH = ':'.join(p for p in os.environ['PATH'].split(':') if '/.local/bin' not in p)  # no codegraph CLI
def run(task, arm, i):
    out = f'{B}/codex/{task}/{arm}-{i}.jsonl'
    if os.path.exists(out) and '"turn.completed"' in open(out).read(): return
    os.makedirs(os.path.dirname(out), exist_ok=True)
    cwd = prepare(task, f'codex-{arm}-{i}')
    prompt, mcp = ARMS[arm]
    args = ['codex', 'exec', '--json', '--skip-git-repo-check', '--dangerously-bypass-approvals-and-sandbox',
            '-c', f'developer_instructions={toml(prompt)}', '-c', 'mcp_servers={}']
    if mcp:
        args += ['-c', f'mcp_servers.codegraph.command={toml(CG)}', '-c', f'mcp_servers.codegraph.args={json.dumps(["serve","--mcp","--path",cwd])}',
                 '-c', 'mcp_servers.codegraph.env={CODEGRAPH_TELEMETRY="0", CODEGRAPH_NO_UPDATE_CHECK="1"}', '-c', 'mcp_servers.codegraph.tool_timeout_sec=120']
    args.append(TASKS[task][2])
    t = time.time()
    with open(out, 'w') as f:
        try: subprocess.run(args, cwd=cwd, env={**os.environ, 'PATH': PATH}, stdout=f, stderr=open(out[:-6] + '.err', 'w'), stdin=subprocess.DEVNULL, timeout=1800)
        except subprocess.TimeoutExpired: pass
    f = open(out, 'a'); f.write(json.dumps({'type': 'bench.wall', 'seconds': time.time() - t}) + '\n'); f.close()
    if TASKS[task][1]:
        g = lambda *a: subprocess.run(['git', *a], cwd=cwd, capture_output=True, text=True).stdout
        open(out[:-6] + '.diff', 'w').write(g('diff', '--stat') + g('status', '--porcelain') + '\n' + g('diff') + '\nUNTRACKED:\n' + g('ls-files', '--others', '--exclude-standard'))
    print(f'{task} {arm}-{i} done in {time.time()-t:.0f}s', flush=True)
if __name__ == '__main__':
    jobs = [tuple(j.split(':')) for j in sys.argv[2:]]
    with cf.ThreadPoolExecutor(int(sys.argv[1])) as ex:
        for f in cf.as_completed([ex.submit(run, t, a, int(n)) for t, a, n in jobs]): f.result()
