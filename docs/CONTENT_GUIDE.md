# Content Guide

Status: placeholder — the real guide lands with milestone M2 (content pipeline, plan
§14). Until then, [`plan.md`](../plan.md) §10 is the reference:

- §10.1 — the boundary table: what lives in data vs. code.
- §10.2 — authoring units: everything integer (ms, milli-tiles, plain counts); every
  file starts with `schema_version`.
- §10.3 — an example RON entity record.
- §10.4 — the Alpha manifest and starter stats.
- §10.5 — Alpha map requirements.
- §10.6 — the add-a-unit / add-a-map acceptance tests.

When M2 lands, this file should explain: file layout under `content/`, the full
schema with field-by-field units and ranges, how to add a unit/faction/map without
touching engine code, and how validation errors read.
