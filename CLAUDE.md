# Lessons for future Claude sessions on this repo

## Don't bail on "lots of work"

N hand copies / N-day implementation = wrong-shape signal. Find parametric / generic / macro shape that makes the work O(1). "5 days of typing N near-duplicates" = failure mode, not workload.

## Time estimates are ~100x too long

Tasks framed as "~5 days" usually take hours. Discount aggressively. Commit + iterate, don't pre-emptively scope down.

## First principles over brute force

2000-line unrolled implementation / 100-step chain = problem statement too narrow. Reshape the problem, not the labor.

## Implementation flow

**File-disjoint per component**:
1. Plan first: per-component sections (Purpose / API / Behavior / gaps) in a plan doc. Iterate plan with codex review until READY.
2. `general-purpose` agents per component, `isolation: "worktree"`, `run_in_background: true`. Each: implement → simplify → commit. Do NOT have agents call codex-rescue (stalls watchdog).
3. Commit is mandatory for general-purpose agents — say so explicitly + verify with `git log -1 --oneline` before returning.
4. Orchestrator-level codex review per-component after commits land. Tight prompts.
5. Holistic review across components for cross-cutting issues (interface drift, duplication, pattern divergence).
6. PR off branch.

**File-shared phased work**:
1. Plan locks every phase boundary. Adversarial codex review × 2 before execution. Pin architectural decisions as numbered "Decision N" entries the executor cites verbatim.
2. One codex per phase. Codex edits, orchestrator commits (codex sandbox blocks `.git`).
3. Split phases >500 LOC preemptively into Xa/Xb.
4. Build green = only fitness signal. No partial-phase commits.
5. Mid-stream review every 3-4 phases (see "Mid-execution review checkpoints"). Holistic review at plan end.

## Plan file standard

Location: `docs/<TOPIC>_PLAN.md`. ALL_CAPS topic, `_PLAN.md` suffix.

Three sections, in order:
1. **Purpose** — one paragraph. What the plan is about and the user-facing reason it exists. No history, no alternatives considered.
2. **Steps** — numbered list. Each step is a concrete, actionable change citing exact files (and line numbers when stable). Decisions only — no "we could", no tradeoffs, no fallbacks.
3. **End state** — bulleted list of observable post-conditions. Each bullet is a checkable fact about the repo after the plan executes (file deleted, function callable, build green, CLI surface, etc.).

Forbidden in plan files:
- Time estimates ("~1 day", "5 hours").
- Speculation ("might", "could", "if X then maybe Y").
- Pros/cons or tradeoff tables — pick one path and commit.
- Deliberation text — no "we considered X but chose Y".
- Status/progress notes — plans are forward-looking specs, not journals.
- Roadmap / future-work asides — split into a separate plan if needed.

A reader should be able to execute the plan top-to-bottom without making design decisions. If a decision still needs to be made, resolve it before writing the plan.

## Monitor long-running work

Monitor tool watching mtimes / git HEAD / output sizes / PID. Heartbeat 5-15min for long builds, 30-60s for build / process-liveness. Two monitors: commit-watcher + per-task PID-watcher. Cross-check ledger PID with `kill -0 <pid>` — stale entries fire DONE prematurely.

## Code comments = invariants, not history

No phase / commit hash / session names in code. WHY-historical → commit message. Default = no comment.

## Document what exists, not what's coming

"Production X will…" / "future Y swap-in…" = overreach. Roadmap belongs in plan docs, not READMEs. Placeholder = state plainly + stop.

## Codex queue saturation = stale-ledger contention

No real queue. Single-lock broker daemon (`app-server-broker.mjs`) + per-workspace JSON job ledger. Orphan entries (status `running`, dead PID) make new invocations stall.

Triage:
1. `ps aux | grep -i codex` — live process? If no, "queue" entirely stale.
2. `cat ~/.claude/plugins/data/codex-openai-codex/state/<workspace>/state.json` — filter `status:running`, check each PID with `kill -0 <pid>`.
3. Nuke: `rm -rf ~/.claude/plugins/data/codex-openai-codex/state/<workspace>/ ; pkill -f app-server-broker.mjs`.

Routing by task type:
- Read-only review → in-orchestrator Read/Grep.
- Isolated review → Agent + general-purpose.
- Adversarial review → codex:codex-rescue plugin (10min watchdog OK).
- Execution + commits → direct codex CLI:
  ```bash
  codex exec --sandbox workspace-write --skip-git-repo-check \
    --color never -C <repo> --output-last-message <final.txt> \
    < prompt.txt > log 2>&1 &
  ```

## Verify agent commit before trusting report

Subagent reports = narrative, not proof. Run `git status --short` + `git log --oneline` after. If files uncommitted, orchestrator commits with message recording agent scope. Recurring failure mode: agent reports "complete, all gates green" while leaving touched files in working tree.

## Codex plugin can't commit; direct CLI for execution

Plugin sandbox = `workspace-write`, blocks `.git`. Two shapes:
1. Codex edits, orchestrator commits.
2. Direct CLI (above) — same can't-commit, but faster, more visible, bypasses plugin watchdog.

## Codex plugin 10-min watchdog hands off async

codex:codex-rescue subagent ~600s watchdog. Beyond it, returns "task running in background ID X" handle, work continues async, NO completion signal. Reported wall ≈ 10-11min suspect hand-off. Check `state.json` jobs status `running` + `kill -0 <pid>`. Direct `codex exec` sidesteps entirely.

## Network failures during long codex sessions are survivable

WebSocket to chatgpt backend disconnects under sustained load — typically after sessions exceed ~1M tokens or ~60 min wall. Log ends `failed to connect to websocket`, no `--output-last-message`, PID exits clean-less. BUT: file edits intact + build green at disconnect (codex flushes incrementally).

Recovery: confirm green, commit as `<phase>a` partial, spawn `<phase>b` for rest. Prompt always includes "after edits, run the build, return only when green."

## Phases >500 LOC split preemptively

Heuristic: phase touches >5 files OR introduces >20 new public items OR rewrites >20 → ship as Xa/Xb from start. Splitting after-the-fact (post-network-fail / budget-exhaust) works but is more work.

## Codex factors well via shared cores when prompted

Without explicit hint, codex inlines the same logic N times across N call sites. With "factor through a shared helper" prompt, single helper. Use this every time a phase touches multiple modules that share structure.

## Shell var name `status` is read-only in zsh

zsh's `$status` mirrors `$?`, read-only. `status=$(jq ...)` fails immediately. Rename to `jstatus` / `cur_status`.

## Codex per-phase deferrals must bind to follow-up phases

Codex is honest about deferrals in per-phase reports ("X stays as-is", "boundary not fully expanded", "module fails standalone"). Orchestrators commit each "best effort" + move on. Net: deferrals filed in early phases become critical / high findings at the post-lift review.

Hard rule: any "deferral" / "weakened" / "partial" / "deferred" in a codex report → either (a) follow-up phase scheduled before next phase commits, or (b) explicit user-confirmed acceptance with closing path. Parse `Deviations` + `Known deferred` sections from each `/tmp/codex_<phase>_final.md` → TaskCreate + commit message. Don't queue the next phase until deferrals are dispositioned.

## Mid-execution review checkpoints

Plan-review runs before any execution; post-lift review runs after the last phase. With many phases between, two reviews bracket execution with no feedback. Codex is stateless across turns; same drift repeats.

Mitigation: every 3-4 phases, read-only codex review. Scope: cross-phase consistency, compounding deferrals, interface drift, weak abstractions. Signal not edit — produces a list the orchestrator either schedules as follow-ups or explicitly accepts before continuing.

For 8+ phase runs, budget ~ceiling(N/3) mid-stream passes × ~30min each. Cheap vs landing critical findings post-lift.

## Persistent codex sessions via `codex exec resume`

Multi-phase orchestration: don't spawn fresh sessions every phase. Resume.

Storage: `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<UUID>.jsonl`. Each turn appends. Header = `session_meta` event with UUID.

Workflow:
```bash
# initial turn
codex exec --sandbox workspace-write --skip-git-repo-check \
  --color never -C <repo> --output-last-message /tmp/turn1.txt \
  < prompt1.txt > log1.txt 2>&1

# capture UUID
UUID=$(grep -m1 '"session_meta"' log1.txt | jq -r '.payload.id')

# resume
codex exec resume "$UUID" --output-last-message /tmp/turn2.txt \
  < prompt2.txt > log2.txt 2>&1
# or
codex exec resume --last < prompt2.txt
```

Codex keeps prior reasoning, tool-call results, file reads. Phase N+1 prompts terse ("close deferral from N you flagged") instead of 3000-word context rebuilds. Mitigates stateless-across-turns drift.

Caveats: long sessions hit compaction (lossy); network failure may corrupt session ledger (fallback = fresh session); plugin variant doesn't share resume — direct CLI only.

When NOT to resume: independent review / simplifier / adversarial pass — fresh context preferred.
