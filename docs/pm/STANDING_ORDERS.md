PANDEMONIUM — STANDING ORDERS v1.1 (from the PM; apply to every task until superseded)

1. CHAIN OF COMMAND
The PM assigns work via written briefs relayed by the owner. Do only what the brief says; never self-assign. You do not talk to the other agent. If you need something from their lane, list it under "Cross-lane requests" in your report and the PM will route it.

2. CONSTITUTION
plan.md §2 (FD-1..FD-10), §5 (determinism), §18 (dangerous shortcuts) and §19 (invariants) stay absolute. Changing any of them needs an ADR approved by PM + owner; never silently.
AMENDMENT: plan.md §0's "foundation, not a game / no content or polish" clause and §1.6 anti-goals were scoped to the Alpha. For any brief marked PRODUCT PHASE they are suspended: menus, audio, art, packaging and content are allowed. Architecture rules still bind that work (UI never mutates the sim, no floats in sim, content stays data).
AI-Handoff.md and plan.md are edited only when a brief says so.

3. LANES
Agent A owns: crates/client, crates/engine, README.md, docs/product/, .github/workflows/release.yml.
Agent B owns: crates/{fx,sim,sim_api,content,ai,replay,tools}, tests/, content/, .github/workflows/ci.yml and nightly.yml, docs/pm/.
Do not edit the other lane, except a minimal mechanical change needed to keep the build green; flag it in the report.

4. GIT
Never push to master. Branches: a/<task-id>-<slug> or b/<task-id>-<slug>. One logical change per commit, existing message style. Rebase on latest master before reporting done. The owner merges. If you cannot push, say so and give the owner exact commands. Cargo.lock conflicts: take master's and regenerate with cargo; never hand-merge.

5. REGISTER ID RANGES (prevents merge collisions)
Agent A: assumptions A-100.., debt DEBT-100.., ADR 0100... Agent B: A-200.., DEBT-200.., ADR 0200.. Existing IDs untouched.

6. DEFINITION OF DONE
Green in both profiles: cargo fmt --all -- --check; cargo clippy --workspace --all-targets -- -D warnings; cargo test --workspace; cargo test --workspace --release. Plus every acceptance criterion in the brief, each with evidence (command + output, file path, or run URL). Registers updated. Report delivered in the format in section 10.

7. GOLDEN HASHES
No golden (demo, flagship, content hash, map id) changes unless the brief authorizes it. If one moves unexpectedly, STOP and report old/new values and the moving commit. If authorized, move it in one dedicated commit with the reason in the message.

8. HONESTY
Prove, don't assert. If you could not run something (no GPU, display, network, CI access), say exactly that. "Not verified" is an acceptable status; claiming verification you did not perform is the worst outcome. Tests prove determinism, not feel: anything about look, controls or feel is "needs owner eyes" until a human confirms it.

9. SCOPE AND STOP CONDITIONS
Discoveries outside the brief go under "Proposals"; do not implement them. STOP and report immediately if: a frozen decision looks wrong; a golden moves unexpectedly; the architecture-law test would need weakening; cross-OS hashes diverge; the brief conflicts with plan.md.

10. REPORT FORMAT (every task)
TASK ID / STATUS (done | partial | blocked)
1 Summary (max 5 lines) · 2 Changes (branch, commits, files) · 3 Evidence per acceptance criterion · 4 Gate results (test counts, both profiles) · 5 Goldens (unchanged, or moved + why) · 6 Deviations and assumptions (IDs) · 7 New debt (IDs) · 8 Risks and surprises · 9 Proposals (not implemented) · 10 Cross-lane requests · 11 Needs from owner

11. MERGE PROTOCOL (added in v1.1; binds every branch, every rebase, and every PR)
MP-1 Touch list: every brief declares the files it may modify. The report ends with the actual touch list (`git diff --name-only master...HEAD`) set against the declared one.
MP-2 Parallel briefs run only if their touch lists are disjoint; otherwise the PM serializes them and states the merge order.
MP-3 Registers and shared docs (DEBT, ASSUMPTIONS, CHANGELOG, AI-Handoff, plan, the ADR index) change only if the brief authorizes it, in ONE final commit, redone — not merged — after any rebase.
MP-4 Rebase; never merge master into the branch. Conflicts are resolved only on the agent's own branch, and force-push goes only with --force-with-lease to that branch. Never resolve anything in GitHub's web editor.
MP-5 One PR merges at a time. After each merge the PM names which open branches must rebase before merging. Reports state the master SHA they are rebased on.
MP-6 Push early: the first push right after branch creation, then after every commit that passes fmt+clippy. A failed push means STOP and report.
MP-7 No edits to the other lane's files unless the brief names the file. Cross-lane needs go in report section 11.
MP-8 The only required status check on master is `ci-required`.
