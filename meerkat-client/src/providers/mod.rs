//! Per-provider runtime implementations. Feature-gated per existing
//! `anthropic`/`openai`/`gemini` features.

#[cfg(feature = "anthropic")]
pub mod anthropic;

#[cfg(feature = "openai")]
pub mod openai;

#[cfg(feature = "gemini")]
pub mod google;
