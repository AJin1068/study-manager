//! PDF text extraction is optional. No parser crate is bundled yet (adding one is a dependency
//! decision recorded in docs/decision-llm-egress.md), so the default extractor answers
//! `Unsupported` and the UI says so. A real extractor plugs in through `PdfExtractor`; whatever it
//! is, it runs on its own thread so a parser panic cannot take the app down.

use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PdfError {
    /// No extractor is installed.
    #[error("pdf_unsupported")]
    Unsupported,
    /// The file has no text layer (a scan): extraction would need OCR, which is out of scope.
    #[error("pdf_no_text")]
    NoText,
    #[error("pdf_failed")]
    Failed,
    /// The parser panicked; the panic was contained.
    #[error("pdf_parser_panicked")]
    Panicked,
}

impl PdfError {
    pub fn code(self) -> &'static str {
        match self {
            PdfError::Unsupported => "pdf_unsupported",
            PdfError::NoText => "pdf_no_text",
            PdfError::Failed => "pdf_failed",
            PdfError::Panicked => "pdf_parser_panicked",
        }
    }
}

pub trait PdfExtractor: Send + Sync {
    fn extract(&self, bytes: &[u8]) -> Result<String, PdfError>;
}

pub struct NoPdfExtractor;

impl PdfExtractor for NoPdfExtractor {
    fn extract(&self, _bytes: &[u8]) -> Result<String, PdfError> {
        Err(PdfError::Unsupported)
    }
}

/// Runs the extractor on a separate thread and turns a panic into `PdfError::Panicked`.
pub fn extract_isolated(
    extractor: &Arc<dyn PdfExtractor>,
    bytes: Vec<u8>,
) -> Result<String, PdfError> {
    let extractor = Arc::clone(extractor);
    let worker = std::thread::Builder::new()
        .name("pdf-extract".into())
        .spawn(move || extractor.extract(&bytes))
        .map_err(|_| PdfError::Failed)?;
    match worker.join() {
        Ok(result) => match result {
            Ok(text) if text.trim().is_empty() => Err(PdfError::NoText),
            other => other,
        },
        Err(_) => Err(PdfError::Panicked),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) struct Panicking;

    impl PdfExtractor for Panicking {
        fn extract(&self, _bytes: &[u8]) -> Result<String, PdfError> {
            panic!("parser blew up on a malformed file");
        }
    }

    struct Fixed(&'static str);

    impl PdfExtractor for Fixed {
        fn extract(&self, _bytes: &[u8]) -> Result<String, PdfError> {
            Ok(self.0.to_string())
        }
    }

    #[test]
    fn the_default_extractor_says_unsupported() {
        let extractor: Arc<dyn PdfExtractor> = Arc::new(NoPdfExtractor);
        assert_eq!(
            extract_isolated(&extractor, b"%PDF-1.4".to_vec()),
            Err(PdfError::Unsupported)
        );
    }

    #[test]
    fn a_parser_panic_is_contained_and_reported() {
        let extractor: Arc<dyn PdfExtractor> = Arc::new(Panicking);
        assert_eq!(
            extract_isolated(&extractor, vec![1, 2, 3]),
            Err(PdfError::Panicked)
        );
        // The process is still here and can extract again.
        let ok: Arc<dyn PdfExtractor> = Arc::new(Fixed("본문"));
        assert_eq!(extract_isolated(&ok, Vec::new()).as_deref(), Ok("본문"));
    }

    #[test]
    fn a_scan_without_text_is_reported_not_ingested_as_empty() {
        let scan: Arc<dyn PdfExtractor> = Arc::new(Fixed("  \n "));
        assert_eq!(extract_isolated(&scan, Vec::new()), Err(PdfError::NoText));
    }
}
