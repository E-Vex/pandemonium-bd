//! Replay recording and verification (plan §6.5): a match is seed + content hash +
//! ordered command log; checkpoints carry `(tick, state hash)` pairs.
//!
//! The replay file is `{ format_version, content_hash, map_id, seed, player_setup,
//! commands, checkpoints, final_hash }`. `tools replay-verify` re-simulates a replay
//! headlessly and compares every checkpoint hash (acceptance A2). This crate depends
//! only on `fx` and `sim_api` (plan §4); the re-simulation itself is orchestrated by
//! `tools`, which depends on `sim`.

#![forbid(unsafe_code)]
