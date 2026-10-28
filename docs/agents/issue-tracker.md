# Issue tracker: local Markdown

Issues and specs live in `.scratch/`.

- Keep each feature in `.scratch/<feature-slug>/`.
- Keep its spec in `spec.md`.
- Publish one ticket per `issues/<NN>-<slug>.md`, numbered from 01
  in dependency order. Keep ticket numbers stable.
- Record status near the top using the roles in `triage-labels.md`.
- Record blocking tickets under `Blocked by`, using their numbers
  and titles. Use "None" when there are no blockers.
- Append discussion under `## Comments`.

When a skill says to publish a ticket, create its individual issue
file. When it says to fetch a ticket, read the referenced file.

Work can start when all blocking tickets are complete. Record
completion explicitly in the ticket; readiness alone does not
mean completion.

Scratch files remain local and excluded from source distributions.
