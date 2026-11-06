//! LLM Wiki: one markdown folder per subject (`fs`), a rebuildable index (`index`), ingest of
//! user material into page proposals that are applied only after a preview (`ingest`), and
//! questions answered from a subject's own pages (`ask`, `retrieve`). `service::WikiService` ties
//! them to the database and the LLM gate.
//!
//! The wiki works without any LLM: subjects, pages, indexing, search, and the heading-based
//! fallback import are local. The LLM only drafts and answers, and every draft is validated and
//! previewed before a file changes.

pub mod ask;
pub mod fs;
pub mod index;
pub mod ingest;
pub mod lint;
pub mod names;
pub mod pdf;
pub mod recommend;
pub mod retrieve;
pub mod retro;
pub mod retro_service;
pub mod service;
pub mod topic;
