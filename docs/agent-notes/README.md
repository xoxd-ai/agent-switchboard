# Agent notes

This directory is the durable, git-tracked place for agent working output in
this repo. Scratchpads and `/tmp` get wiped without warning. Anything another
session, or another host after `git pull`, would need goes here:

- investigation notes and verdicts;
- phase receipts (R-N13: every phase writes one, and so does every session
  before it ends);
- plans, checklists and unpushed-work manifests.

Distilled rulings do not go here. They belong in a dated TIN-4655 comment and
then in the ADR that owns the area: [ADR-0001](../adr/0001-agent-switchboard.md)
for the broker (`SWB-R01`..`SWB-R24`, `SWB-R49`..`SWB-R58`) or [ADR-0002](../adr/0002-lgtm-plane.md)
for the LGTM plane (`SWB-R25`..`SWB-R48`, including the September 26 cross-plane amendments). Secret-bearing scratch goes to
`~/.claude/agent-notes-rescue/YYYY-MM-DD/` and never enters the repo.

## Naming

```text
docs/agent-notes/YYYY-MM-DD-<TIN-####|sess-<slug>>-<topic>.md
```

Use the `TIN-####` token when a Linear issue exists. Use one topic per file.
The date is the creation date in UTC.

## Frontmatter

Every note opens with:

```yaml
---
title: "P1a substrate receipts"      # at most 120 characters
date: 2026-09-25                     # must equal the filename prefix
status: active                       # active | landed | archived
summary: >-                          # at most 240 characters
  One scannable sentence on what the note establishes.
refs:                                # may be []
  - TIN-4655
---
```

## Retiring a note

Delete a note once its decisions have durable carriers (the ADR, a Linear
comment) and nothing still depends on it. Do it in a signed commit that names
what replaces it. Git history is the archive, so never create an `archive/`
directory. For a note you keep, add a dated correction instead of silently
rewriting its evidence.
