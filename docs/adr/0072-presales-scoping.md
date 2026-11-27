# ADR-072: Presales scoping drafts a templated Scope of Work with a workflow, keeps people's edits, and travels with the quote

- **Status:** Accepted (implementer's design)
- **Date:** 2026-11-27

## Context

Phase 10's presales scoping is a workspace on an opportunity. Discovery feeds three AI-drafted artifacts: a Scope of Work, a work breakdown structure and a bill of materials. People edit and approve them, and they then assemble the quote. This first part delivers discovery and the Scope of Work. The WBS, the BOM and quote assembly follow.

## Decision

- **One scope per opportunity**, at the client, following a **template**: project, managed services onboarding, migration or assessment.
  - Templates are code for now. Each lists its sections, with guidance for people and the AI.
  - A scope has discovery **sources** (notes, transcripts, documents' text), **sections**, **open questions** and approved **versions**.
- **Drafting is a workflow**, as for every AI feature (§8.8).
  - The API raises `scope.draft_requested`.
  - The built-in **Presales scoping** pack reads it with `core.scope.context`: the numbered discovery, the client's environment (contacts, sites, devices by type) and agreements, past approved scopes of won work of the same kind (structure only), sections people already wrote, and answered questions. It drafts with `ai.extract` and keeps the draft with `core.scope.write`.
  - The prompt forbids inventing facts and turns gaps into open questions. Citations are source numbers, mapped back to labels.
  - A section someone has edited is never overwritten.
  - Failures land on the scope (`failed`, with the error) through the workflow's `on_error`.
  - The workspace polls while a draft is requested, and can stop waiting, for example when the pack isn't installed.
- **Approving** snapshots the non-empty sections as the next version. A quote on the same opportunity can carry an approved version (`scope_version_id`). The quote then includes it: it's in the revision snapshot when sent, so the portal signature's SHA-256 covers it, and it appears in the portal view and the PDF.

## Consequences

- The WBS and BOM (next) hang off the same scope and discovery, and quote assembly turns them into lines.
- Uploaded files aren't read automatically yet: their text is pasted as a document source. Text extraction from attachments can feed the same table.
- Organizations can tune the prompt, or switch models, by copying the pack's workflow.
