# RFC-0268: RFCs in the open

- Start date: 2026-10-28
- Authors: jmoo
- RFC PR: [jmoo/drawbar#268](https://github.com/jmoo/drawbar/pull/268)
- Tracking issue: none

## Summary

Design decisions are proposed, reviewed and recorded as RFCs in this repository:
Markdown files in `rfc/`, each numbered by the pull request that proposes it, and
accepted by merging that pull request. The rules are the RFCs section of
`CONTRIBUTING.md`, changed in the same pull request. This is the first RFC to
follow them.

## Motivation

The project's design decisions so far are RFCs kept in the maintainer's private
notes. Issue and pull request titles cite them (`rfc-0015`, `rfc-0029`), but the
documents are not in this repository. That worked while one person wrote the code.

There are now outside contributors. They can read that a decision was made but not
why, and they cannot propose, review or amend a decision where the project can see
it. The first outside proposal, building piano libraries from a recorded take,
arrived by email because it had nowhere public to go. Contributors who work with
coding agents hit the same wall: an agent can read the repository but not the
notes.

## Guide-level explanation

A contributor with a decision to propose copies `rfc/0000-template.md`, writes it,
and opens a pull request with the `rfc` label. Once the pull request has a number,
they rename the file to `rfc/NNNN-short-title.md` and fill in the header.

Review happens on the pull request. The maintainer merges it to accept the RFC, or
closes it with a comment that says whether it was rejected or withdrawn. Work that
implements an accepted RFC cites it in issue and pull request titles, such as
`feat: rfc-0268 …`, and a tracking issue linked from the header follows that work.

When a decision changes, a new RFC proposes the change. Its pull request adds
`Superseded by RFC-NNNN` under the old RFC's title, which is the only edit a
merged RFC receives.

## Reference-level explanation

- **Scope.** An RFC is for a decision that is hard to reverse, crosses crates, or
  has real alternatives worth keeping. Anything else is an issue or a pull request.
- **Files.** `rfc/NNNN-short-title.md`, where `NNNN` is the proposing pull
  request's number padded to four digits. A number never changes and is never
  reused.
- **Header.** Start date, authors, RFC PR and tracking issue, as a list under the
  title. There is no status field.
- **Sections.** Summary, Motivation, Guide-level explanation, Reference-level
  explanation, Drawbacks, Rationale and alternatives, Prior art, Unresolved
  questions, Future possibilities.
- **Decisions.** Merged is accepted. Closed is rejected or withdrawn, recorded in
  the closed pull request and its closing comment.
- **After merge.** A merged RFC records a decision; the code and `docs/` describe
  the current state. The only later edit is the supersede line.
- **Evidence.** Aggregate facts measured on the private corpus may be stated, as
  pull request descriptions already do. Specimens, their paths and anything
  proprietary stay out.
- **Citations.** Titles of implementing issues and pull requests cite the RFC.
  Code and comments do not.

## Drawbacks

- Rejected RFCs are not in the tree. Their record is a closed pull request, which
  is less visible than a file and depends on GitHub.
- A merged RFC can read as current after the code has moved on. The supersede line
  covers replaced decisions; it does not cover drift without a new RFC.
- A number is known only after the pull request opens, so the file is renamed once.

## Rationale and alternatives

| Option | Why not |
|---|---|
| Keep RFCs private, and the maintainer transcribes outside proposals | Contributors cannot see rationale, and every proposal goes through one person |
| Publish every RFC, old and new | The existing RFCs hold corpus paths, specimen names and private notes, and cleaning them is a large job |
| GitHub issues or discussions | Only the author and the maintainer can edit the text, revisions are not diffable, and the decision is not part of the tree |
| GitHub wiki | Edits land without review, anyone with write access can change a decided page, and a clone of the repository does not include it |

A pull request is already where this project reviews text, so the RFC lands in the
tree it describes, with its discussion attached.

**Numbering.** Continuing the private sequence lets two open pull requests claim
the same number. The pull request number is unique without coordination and higher
than any existing RFC number, at the cost of gaps.

**No status field.** Merging is the acceptance, so a status would repeat it.
Whether an RFC is implemented changes over time and belongs to its tracking issue.
A merged rejection, with a `rejected` status, would keep the record in the tree,
but it means merging text the project decided against.

**Template.** The Rust project's RFC template splits a proposal into what a user
sees and how it is built, which matches this repository's split between the guide
and Reference in `docs/`. It also has places for prior art and unresolved
questions, which reverse-engineering proposals need. The template here keeps its
sections and writes its own guidance.

## Prior art

- The Rust project's RFC process ([rust-lang/rfcs](https://github.com/rust-lang/rfcs)):
  RFCs as files reviewed by pull request, numbered by that pull request, accepted
  by merge, and followed by tracking issues.
- This project's private RFCs, which used Problem, Constraints, Options,
  Recommendation and Consequences, a status field, and kept rejected RFCs.

## Unresolved questions

None.

## Future possibilities

- The recorded-take proposal moves here as the first contributor RFC.
- A check that every file in `rfc/` matches its pull request number and carries the
  header.
