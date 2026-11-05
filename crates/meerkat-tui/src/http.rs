//! Bounded HTTP body reads for the TUI's own requests (model lists, the
//! community price list): the body is read chunk by chunk and refused once
//! it passes a cap, so a missing or lying `Content-Length` cannot exhaust
//! memory.

/// A body read that failed or passed its cap.
#[derive(Debug, thiserror::Error)]
pub enum CappedReadError {
    /// Transport failure (the URL is stripped).
    #[error("network error: {0}")]
    Http(#[source] reqwest::Error),
    /// The body is larger than the cap.
    #[error("response larger than {cap} bytes")]
    TooLarge {
        /// The cap in bytes.
        cap: usize,
    },
}

impl From<reqwest::Error> for CappedReadError {
    fn from(err: reqwest::Error) -> Self {
        Self::Http(err.without_url())
    }
}

/// Read `response`'s body, at most `cap` bytes.
///
/// # Errors
///
/// [`CappedReadError::TooLarge`] when the declared or the actual length
/// passes `cap`, [`CappedReadError::Http`] on transport failures.
pub async fn read_capped(
    mut response: reqwest::Response,
    cap: usize,
) -> Result<Vec<u8>, CappedReadError> {
    if response
        .content_length()
        .is_some_and(|len| len > cap as u64)
    {
        return Err(CappedReadError::TooLarge { cap });
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > cap {
            return Err(CappedReadError::TooLarge { cap });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
