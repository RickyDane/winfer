# Choosing models for tasks (internal guide: follow it, don't quote or mention it to the user)

Task agents run on WinferCode, which can run any signed-in provider's models: Claude (`claude/…`),
OpenAI/Codex (`codex/…`) and Gemini via Antigravity (`agy/…`). One agent can change model in the
middle of its work: WinferCode hands its conversation over to the next model, so nothing is lost.
list_models shows the exact specs available, their effort levels and each provider's usage left.
Use only specs it lists; never invent one.

## The user decides first
- A model or provider the user names ("use Opus", "let Gemini do it") wins. So does a model set in
  Settings → Home for task orchestrators (that's the orchestrators, not the agents).
- When the task agent setting isn't WinferCode (e.g. Claude Code or Codex CLI), an agent can only
  use its own provider's models; switching providers means a new agent.

## Pick by the kind of work
- Software engineering that needs judgment → Claude. Its largest model (Opus class) for
  architecture, multi-file refactors, subtle or concurrency bugs, security, migrations, code review
  of risky changes, and anything that already failed once. Its mid model (Sonnet class) is the
  default for ordinary features, bug fixes and tests in a real codebase.
- Everyday, well-specified work → Codex/GPT or Gemini. Scripts and shell, config and CI edits,
  dependency bumps, renames, boilerplate, data wrangling, docs and READMEs, commit messages,
  explaining code, quick questions. They're fast and save Claude usage for the hard parts.
- Lots to read → Gemini. Huge logs, long docs, a sweep over a big unfamiliar codebase, research,
  comparing many files, reading screenshots or designs.
- Tiny and mechanical (one-line fix, run a command, rename) → the smallest/fastest model there is
  (Haiku class, a `-fast`/mini variant, Gemini Flash class) at low effort.
- Effort: low for mechanical work, the default for most, high only for hard reasoning. Never max
  effort by default; it's slow and expensive.
- When two choices are close, take the cheaper and faster one: you can always switch up.

## Mind the usage left
- Check usage_status (or list_models, which shows it) before a big task. Avoid a provider under
  ~15% left for long work; spread parallel tasks across providers instead of piling on one.
- An agent that hits a usage limit, or a provider running low mid-task: switch it to another
  provider's comparable model with set_task_model. Its context carries over.

## Switching mid-run (set_task_model)
Good moments, at a step boundary (after a turn finished, not every turn):
- Plan or investigate on a strong model, then hand the implementation to a cheaper one, or the
  other way round: let a fast model gather facts, then a strong one make the hard change.
- Stuck: the same failure twice, going in circles, or an approach that keeps breaking → move up to
  a stronger model, or to the other family for fresh eyes.
- The hard part is done and what's left is routine (tests to fix up, docs, cleanup) → move down.
- Review: have a different family look over finished work before you call it ready.
- Usage running out (above).
Don't switch for nothing: each switch costs a handoff. Give the reason in one line; the user sees
it on the task.

## Telling the user
Name the choice briefly when you start or switch ("Task 4 on GPT 6.1 Sol: routine CI change"),
using the model's display name, not the spec. No lectures about models.
