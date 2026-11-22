# Prompt labels (draft, for the classifier experiment)

Prompts are what a teenager types to an AI coding agent (Claude Code, Codex,
opencode) while building their own project. Each prompt gets exactly ONE act
and exactly ONE task.

## act: what kind of message it is
- `new_request`: asks the agent to do something new (not fixing its last answer).
- `correction`: says the agent's previous work was wrong and must be changed ("no, that's wrong", "that's not what I meant, redo it", "you broke X").
- `clarification`: answers the agent's question or adds details/constraints to the current task ("use postgres", "I meant the left sidebar", "yes the blue one, and also make it round").
- `approval`: just lets the agent continue: "yes", "ok", "continue", "go ahead", "looks good", "lgtm", "do it".
- `question`: wants an answer or explanation, not a change ("why does this happen?", "what is a mutex?").
- `stop_undo`: stop, revert, roll back, undo the last change.
- `handoff`: hands over a huge open-ended job and asks not to be involved ("build the whole app, don't ask me anything", "work on this for hours", "just do everything").
- `chit_chat`: thanks, greetings, jokes, small talk with no task.
- `other`: anything else.

## task: what it is about
- `feature`: building new functionality.
- `bug_fix`: fixing a described bug or wrong behaviour (without a pasted log).
- `error_paste`: the prompt mainly contains a pasted error message, stack trace, compiler output or log.
- `refactor`: restructure, clean up, rename, simplify without changing behaviour.
- `understand_code`: understanding existing code ("what does this function do", "where is X handled").
- `learn_concept`: learning a general concept, not about this code ("what's recursion").
- `ui_styling`: look and layout: colours, fonts, CSS, spacing, animations.
- `tests`: writing, running or fixing tests.
- `setup_deploy`: installing, configuring, building, dependencies, git, CI, deploying, hosting.
- `docs_writing`: README, comments, docs, other prose.
- `planning_design`: architecture, planning, choosing an approach or technology before building.
- `review`: check/review/audit existing work ("is this secure?", "review my code").
- `other`: none of the above (also: chit-chat and approvals with no topic).

Notes: `approval`, `chit_chat` and bare `stop_undo` prompts usually have task `other`
unless they name a topic ("ok, now style the button pink" is new_request + ui_styling).

## Hints for edge cases (same labels, just how to apply them)
- Instructions about how the agent should work, limits, or answers to the agent's own proposal ("you can go a bit over the limit", "use haiku for the agents", "don't commit yet") are `clarification`; if they mostly say "yes, go" they are `approval`.
- "yes, do X" where X was the agent's proposal is `approval`; "yes, and also do Y" (new work) is `new_request`.
- Asking for an estimate, opinion or comparison ("how much work is X?", "would 50 MB help?") is `question` + `planning_design`.
- Pointing out that the agent's last change is wrong or incomplete ("that broke the login", "the label is invisible now") is `correction`, even if phrased as a new request.
- Git, commits, pushing, CI, Docker, servers, installing tools: task `setup_deploy`.
- Research, data or ML experiments, and anything not about building software: task `other` unless another task fits.
