//! The shared-wiki port. The app talks to the server only through this; the one real adapter
//! (`rest::RestSharedWiki`) speaks the Supabase RPCs, and in mock mode the same adapter runs on the
//! in-memory `FakeBackend`.

use super::model::{Published, SharedError, WirePage};

pub trait SharedWikiPort: Send + Sync {
    /// Adds a revision of the caller's text for `page_name`. `base_revision` is the revision the
    /// text was edited from; a publish that does not build on the newest revision is stored as a
    /// branch, never as an overwrite.
    fn publish(
        &self,
        room_id: &str,
        page_name: &str,
        body: &str,
        base_revision: Option<&str>,
        allow_llm_merge: bool,
    ) -> Result<Published, SharedError>;
    /// Tombstones everything of the caller's on the page (and merges crediting them). Returns how
    /// many revisions were withdrawn.
    fn withdraw(&self, room_id: &str, page_name: &str) -> Result<usize, SharedError>;
    fn list(&self, room_id: &str) -> Result<Vec<WirePage>, SharedError>;
    /// Stores a merged page. The server accepts only consented sources and derives the contributor
    /// list from them. Returns the new revision id.
    fn save_merged(
        &self,
        room_id: &str,
        page_name: &str,
        body: &str,
        source_revisions: &[String],
    ) -> Result<String, SharedError>;
}
