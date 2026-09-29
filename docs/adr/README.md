# Architecture Decision Records

An ADR proposes a **change to a frozen foundational decision** (plan §2, FD-1..FD-10)
or to the dependency law (plan §4). It never documents a silent change: the operating
contract (plan §0) requires stopping and writing one, with evidence, *before* the
change is made. Ordinary interpretation gaps belong in
[`../ASSUMPTIONS.md`](../ASSUMPTIONS.md) instead; ordinary simplifications belong in
[`../DEBT.md`](../DEBT.md).

Process:

1. Copy [`TEMPLATE.md`](TEMPLATE.md) to `NNNN-short-title.md` (NNNN = next free
   number, starting at 0001).
2. Fill every section; cite plan sections and test evidence.
3. Link the ADR from the milestone notes in [`../../AI-Handoff.md`](../../AI-Handoff.md)
   and reference it in the commit that lands the change.

Statuses: `proposed` → `accepted` / `rejected` → (later) `superseded by NNNN`.
