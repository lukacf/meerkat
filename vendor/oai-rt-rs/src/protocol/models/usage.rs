use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub total_tokens: u32,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub input_token_details: Option<InputTokenDetails>,
    pub output_token_details: Option<OutputTokenDetails>,
    pub cached_tokens: Option<u32>,
    pub cached_tokens_details: Option<CachedTokenDetails>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputTokenDetails {
    pub cached_tokens: Option<u32>,
    pub text_tokens: Option<u32>,
    pub audio_tokens: Option<u32>,
    pub image_tokens: Option<u32>,
    pub cached_tokens_details: Option<CachedTokenDetails>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputTokenDetails {
    pub text_tokens: Option<u32>,
    pub audio_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedTokenDetails {
    pub text_tokens: Option<u32>,
    pub audio_tokens: Option<u32>,
    pub image_tokens: Option<u32>,
}
