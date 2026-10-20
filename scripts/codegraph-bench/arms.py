CGE = {'CODEGRAPH_TELEMETRY':'0','CODEGRAPH_NO_UPDATE_CHECK':'1'}
HOOK = {'UserPromptSubmit':[{'hooks':[{'type':'command','command':f'env CODEGRAPH_TELEMETRY=0 CODEGRAPH_NO_UPDATE_CHECK=1 {CG} prompt-hook','timeout':30}]}]}
NODE_PROMPT = LEANCG.replace("Treat the source it returns", "To read one symbol you can already name, call codegraph_node instead (much smaller). Treat the source it returns")
ARMS.update({
 'cg_dedup': {'mcp': True, 'env': {'CODEGRAPH_EXPLORE_DEDUP': '1'}},
 'cg_hook': {'mcp': True, 'hooks': HOOK},
 'cg_node': {'mcp': True, 'env': {'CODEGRAPH_MCP_TOOLS': 'explore,node'}, 'prompt': NODE_PROMPT},
})
DS, GK = '/Users/rickyperlick/Coding/bench/deepseek-harness', '/Users/rickyperlick/Coding/bench/grok-build'
EDIT = "Add a new built-in agent tool `count_lines` that returns the number of lines of a file in the workspace (path argument, relative to the workspace root; clear error for missing files). Register it the same way as the existing built-in tools and add tests following the closest existing tool's tests. Don't run builds or tests."
TASKS.update({
 'D1': (DS, False, "How does a user message typed in the Web UI reach the LLM provider, and how does the streamed response get back to the UI? Trace the path with file:line."),
 'D2': (DS, False, "How does the plugin system work: how is a plugin discovered and loaded, and how does it register a tool the agent can call? Cite file:line."),
 'D3': (DS, False, "How are subagents spawned, and how do their results flow back to the parent session? Cite file:line."),
 'DE': (DS, True, EDIT),
 'G1': (GK, False, "How does a tool call requested by the model get executed and its result returned to the model? Trace the path with file:line."),
 'G2': (GK, False, "How does the agent manage the context window: when and how is the conversation history compacted, and what is kept? Cite file:line."),
 'G3': (GK, False, "Trace headless mode from the CLI entry point to the agent loop, and say where it diverges from the interactive TUI path. Cite file:line."),
 'GE': (GK, True, EDIT),
})
SCOUT = {'codegraph-scout': {
  'description': "Finds and explains code through the codegraph index and returns a compact map: the relevant symbols with file:line, how they connect, and short key snippets. Use it for any 'where is / how does / trace' code question instead of reading files yourself.",
  'prompt': "You are a code scout. Answer the question you're given using only codegraph_explore: name the 2-4 symbols or files at its heart (a flow's two ends plus any hop you're unsure of); call it again with more specific names only if a needed hop is missing. Then reply with a compact map, at most ~400 words: each relevant symbol with file:line, one line on what it does, the call path between them in order, and at most 3 short code excerpts (<=8 lines each) only where the exact code matters. Say plainly which hops codegraph couldn't resolve (string-dispatched commands, events, IPC) and give the names to grep for. Don't pad, don't speculate.",
  'tools': ['mcp__codegraph__codegraph_explore'],
  'model': 'sonnet'}}
SCOUT_PROMPT = LEAN.replace("explore with search and reads", "to find or understand code, first ask the codegraph-scout agent (one call; give it the whole question) and work from its map; use grep and short line-range reads only to confirm a detail or fill a gap it names, never re-explore with codegraph yourself")
ROUTE_PROMPT = LEAN.replace("explore with search and reads", "explore with search and reads; for multi-hop questions about how code flows across several files or layers you don't know yet, call codegraph_explore once first, naming the 2-4 symbols at the heart of it, then fill gaps with grep. Skip codegraph for single-file lookups, exact text searches and lists of every use")
ARMS.update({
 'cg_scout': {'mcp': True, 'prompt': SCOUT_PROMPT, 'extra': ['--agents', json.dumps(SCOUT)]},
 'cg_route': {'mcp': True, 'prompt': ROUTE_PROMPT},
})
import copy
SCOUT_H = copy.deepcopy(SCOUT)
SCOUT_H['codegraph-scout']['model'] = 'haiku'
SCOUT_H['codegraph-scout']['prompt'] = SCOUT_H['codegraph-scout']['prompt'].replace("call it again with more specific names only if a needed hop is missing", "make at most 2 calls in total: a second only if a needed hop is missing, naming exactly those symbols")
ARMS['cg_scouth'] = {'mcp': True, 'prompt': SCOUT_PROMPT, 'extra': ['--agents', json.dumps(SCOUT_H)]}
SCOUT_S = copy.deepcopy(SCOUT_H)
SCOUT_S['codegraph-scout']['model'] = 'sonnet'
SCOUT_S['codegraph-scout']['prompt'] = SCOUT_S['codegraph-scout']['prompt'].replace("Don't pad, don't speculate.", "Mark every hop [seen] if its code is in what codegraph returned, or [inferred] if you're guessing from names; never present an inferred hop as fact, and leave out code that nothing calls. Don't pad.")
SCOUT_PROMPT_V = SCOUT_PROMPT.replace("use grep and short line-range reads only to confirm a detail or fill a gap it names", "check its [inferred] hops and the gaps it names with targeted grep and short line-range reads (batched in one call where you can), and trust its [seen] hops")
ARMS['cg_scouts'] = {'mcp': True, 'prompt': SCOUT_PROMPT_V, 'extra': ['--agents', json.dumps(SCOUT_S)]}
SIB = " When you add something modeled on an existing sibling (a tool, command, setting, route), grep every reference to that sibling's name across the repo (code, tests, docs, generated lists) and mirror each one before you finish."
ARMS['plain_sib'] = {'prompt': LEAN.replace("Read a file before editing it,", "Read a file before editing it," + SIB.replace(" When", " when", 1)[:0] + "") + SIB}
ARMS['cg_sib'] = {'mcp': True, 'prompt': LEANCG + SIB}
