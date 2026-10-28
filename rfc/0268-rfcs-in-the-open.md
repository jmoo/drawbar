---
rfc: 268
title: RFCs in the open
authors: [jmoo]
status: proposed
created: 2026-10-28
decided:
superseded_by:
---

# RFC-0268: RFCs in the open

No decision has been made. This RFC proposes how RFCs work from now on, and it is
the first one to follow its own proposal. The rules it would adopt are the RFCs
section of `CONTRIBUTING.md`, changed in the same pull request.

## Problem

The project's design decisions so far are RFC-0001 to RFC-0037, kept in the
maintainer's private notes. Issue and pull request titles cite them (`rfc-0015`,
`rfc-0029`), and `CONTRIBUTING.md` keeps them out of this repository. That worked
while one person wrote the code.

There are now outside contributors. They can read that a decision was made but
not why, and they cannot propose, review or amend a decision where the project
can see it. The first outside proposal, building piano libraries from a recorded
take, arrived by email because it had nowhere public to go. Contributors who work
with coding agents hit the same wall: an agent can read the repository but not
the notes.

## Constraints

- The repository stays public. Personal information, private notes, the private
  corpus and proprietary material stay out of it, as `CONTRIBUTING.md` already
  requires.
- An RFC carries its own evidence. Much of the evidence comes from the private
  corpus, so a public RFC must be able to state what was measured without
  publishing what it was measured on.
- The process stays cheap. RFCs are for decisions that are hard to reverse, cross
  crates, or have real alternatives. Routine work stays in issues and pull
  requests.
- Citations already in issues and commit titles keep their meaning.
- The maintainer makes the decisions.

## Options

| # | Option | Cost | Key tradeoff |
|---|---|---|---|
| A | Keep RFCs private; the maintainer transcribes outside proposals | Low | Contributors cannot see rationale, and every proposal goes through one person |
| B | Publish every RFC, old and new | High | 0001 to 0037 hold corpus paths, specimen names and private notes; cleaning them is a large job and breaks the public-repository rule until done |
| C | New RFCs are Markdown files in `rfc/`, reviewed as pull requests; old ones stay private | Low | Two places to look until the old ones stop mattering |
| D | New RFCs are GitHub issues or discussions | Low | Only the author and the maintainer can edit the text, revisions are not diffable, and the decision is not part of the tree |
| E | New RFCs are pages in the GitHub wiki | Low | Edits land without review, anyone with write access can change a decided page, and a clone of the repository does not include it |

Numbering for C:

| # | Scheme | Tradeoff |
|---|---|---|
| C1 | Continue the private sequence from 0038 | Two open pull requests can claim the same number, so the maintainer reconciles before merging |
| C2 | Use the number of the pull request that proposes the RFC | Unique without coordination, and above every private number, since pull request numbers are already past 260; the sequence jumps from 0037 |

## Recommendation

C with C2: RFCs as files in `rfc/`, numbered by their pull request.

- **A pull request is already the place this project reviews text.** Line comments,
  revisions and the merge are the discussion and the decision, and the RFC lands
  in the tree it describes.
- **Merging records the decision either way.** A rejected RFC is merged with that
  status, because the record of what was not done, and why, is most of what an RFC
  is worth. A pull request closed without merging is a withdrawn proposal.
- **Statuses** are `proposed`, `accepted`, `rejected`, `implemented` and
  `superseded`. A merged RFC changes only its status. A changed decision is a new
  RFC that supersedes the old one.
- **Evidence** is stated as facts and aggregates, which pull request descriptions
  already do. Specimens and their paths stay private.
- **Old RFCs stay where they are.** The maintainer may publish one under its
  original number when a contributor needs it.
- **Code does not cite RFC numbers.** Titles of the issues and pull requests that
  implement an RFC do.

What would change this: if pull requests prove too heavy for proposals, D can carry
early discussion, with the decision still landing as a file.

## Consequences

- `CONTRIBUTING.md` gains an RFCs section and drops the rule that RFCs are private.
  `CLAUDE.md` and `AGENTS.md` link to it, so coding agents follow the same rules.
- An RFC's number is known only once its pull request is open. The author opens the
  pull request, then names the file.
- The maintainer's notes keep research and private remarks. They no longer hold
  the decisions contributors need.
- The first contributor RFC, building piano libraries from a recorded take, moves
  here as its own pull request.
- Acceptance test: this RFC merges as `accepted`, and the next RFC follows it.
