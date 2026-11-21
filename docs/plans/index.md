# Plan index

One row per master plan, oldest first. The phases of a plan are
tracked in that plan's own Execution table, never here.

The `Status` column holds exactly one term from the vocabulary in
`PLAN-TEMPLATE.md`: `Proposed`, `Not started`, `In progress`,
`Blocked`, `Complete`, `Abandoned` or `Superseded`. It is the
whole-plan status, so it only reaches `Complete` once every phase
has been completed, abandoned or superseded.

| Date | Plan | Intent | Status |
|------|------|--------|--------|
| 2026-11-21 | [Teneral: implementation](PLAN-teneral-implementation.md) | Build the daemon, research spikes first, with home IPAM as the first consumer and Shaken Fist after it | In progress |
