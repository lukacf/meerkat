use futures::StreamExt;
use meerkat_core::{ContentBlock, Message, UserMessage, VideoData};
use meerkat_gemini::GeminiClient;
use meerkat_llm_core::{LlmClient, LlmEvent, LlmRequest};
use serde::Deserialize;
use serde_json::Value;
use std::error::Error;
use std::path::Path;
use std::time::{Duration, Instant};

const MODEL: &str = "gemini-3.5-flash";
const PUBLIC_VIDEO_URI: &str =
    "https://storage.googleapis.com/cloud-samples-data/generative-ai/video/pixel8.mp4";
const HSNS_SAMPLE_VIDEO: &str =
    "/Users/luka/src/hsns_clean/data/samples/fragment_08_2_20250831193700.mp4";

fn gemini_api_key() -> Option<String> {
    std::env::var("RKAT_GEMINI_API_KEY")
        .or_else(|_| std::env::var("GEMINI_API_KEY"))
        .or_else(|_| std::env::var("RKAT_GOOGLE_API_KEY"))
        .or_else(|_| std::env::var("GOOGLE_API_KEY"))
        .ok()
        .filter(|key| !key.trim().is_empty())
}

fn video_request(uri: impl Into<String>) -> LlmRequest {
    LlmRequest::new(
        MODEL,
        vec![Message::User(UserMessage::with_blocks(vec![
            ContentBlock::Video {
                media_type: "video/mp4".to_string(),
                duration_ms: 12_000,
                data: VideoData::Uri { uri: uri.into() },
            },
            ContentBlock::Text {
                text: "In one short sentence, describe what is visible in this video.".to_string(),
            },
        ]))],
    )
}

async fn collect_text(
    client: &GeminiClient,
    request: &LlmRequest,
) -> Result<String, Box<dyn Error>> {
    let mut stream = client.stream(request);
    let mut text = String::new();
    while let Some(event) = stream.next().await {
        match event? {
            LlmEvent::TextDelta { delta, .. } => text.push_str(&delta),
            LlmEvent::Done { .. } => break,
            _ => {}
        }
    }
    Ok(text)
}

#[tokio::test]
#[ignore = "live Gemini API smoke; requires GEMINI_API_KEY"]
async fn live_https_video_uri_reference_smoke() -> Result<(), Box<dyn Error>> {
    let Some(api_key) = gemini_api_key() else {
        eprintln!("skipping live smoke: GEMINI_API_KEY is not set");
        return Ok(());
    };
    let client = GeminiClient::new(api_key);
    let text = collect_text(&client, &video_request(PUBLIC_VIDEO_URI)).await?;
    assert!(
        !text.trim().is_empty(),
        "Gemini returned empty text for public HTTPS video URI"
    );
    eprintln!("https video URI smoke response: {}", text.trim());
    Ok(())
}

#[tokio::test]
#[ignore = "live Gemini API smoke; uploads a local video through Files API"]
async fn live_uploaded_file_video_uri_reference_smoke() -> Result<(), Box<dyn Error>> {
    let Some(api_key) = gemini_api_key() else {
        eprintln!("skipping live smoke: GEMINI_API_KEY is not set");
        return Ok(());
    };
    let sample = Path::new(HSNS_SAMPLE_VIDEO);
    if !sample.exists() {
        eprintln!("skipping live smoke: HSNS sample video not found at {HSNS_SAMPLE_VIDEO}");
        return Ok(());
    }

    let file = upload_video_file(&api_key, sample).await?;
    let client = GeminiClient::new(api_key.clone());
    let text = collect_text(&client, &video_request(file.uri.clone())).await?;
    delete_file(&api_key, &file.name).await;
    assert!(
        !text.trim().is_empty(),
        "Gemini returned empty text for uploaded file URI"
    );
    eprintln!("uploaded file URI smoke response: {}", text.trim());
    Ok(())
}

#[derive(Debug, Clone)]
struct GeminiFile {
    name: String,
    uri: String,
    state: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiFileWire {
    name: Option<String>,
    uri: Option<String>,
    state: Option<String>,
}

fn file_from_value(value: Value) -> Result<GeminiFile, Box<dyn Error>> {
    let file_value = value.get("file").cloned().unwrap_or(value);
    let wire: GeminiFileWire = serde_json::from_value(file_value)?;
    Ok(GeminiFile {
        name: wire.name.ok_or("Gemini file response missing name")?,
        uri: wire.uri.ok_or("Gemini file response missing uri")?,
        state: wire.state,
    })
}

async fn upload_video_file(api_key: &str, path: &Path) -> Result<GeminiFile, Box<dyn Error>> {
    let bytes = tokio::fs::read(path).await?;
    let http = reqwest::Client::new();
    let start = http
        .post("https://generativelanguage.googleapis.com/upload/v1beta/files")
        .header("x-goog-api-key", api_key)
        .header("X-Goog-Upload-Protocol", "resumable")
        .header("X-Goog-Upload-Command", "start")
        .header(
            "X-Goog-Upload-Header-Content-Length",
            bytes.len().to_string(),
        )
        .header("X-Goog-Upload-Header-Content-Type", "video/mp4")
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "file": {
                "display_name": "meerkat-live-video-uri-smoke.mp4"
            }
        }))
        .send()
        .await?;
    if !start.status().is_success() {
        return Err(format!(
            "Gemini Files upload start failed: status={} body={}",
            start.status(),
            start.text().await.unwrap_or_default()
        )
        .into());
    }
    let upload_url = start
        .headers()
        .get("x-goog-upload-url")
        .and_then(|value| value.to_str().ok())
        .ok_or("Gemini Files upload start did not return x-goog-upload-url")?
        .to_string();
    let uploaded = http
        .post(upload_url)
        .header("Content-Length", bytes.len().to_string())
        .header("X-Goog-Upload-Offset", "0")
        .header("X-Goog-Upload-Command", "upload, finalize")
        .body(bytes)
        .send()
        .await?;
    let status = uploaded.status();
    let body = uploaded.text().await?;
    if !status.is_success() {
        return Err(format!("Gemini Files upload failed: status={status} body={body}").into());
    }
    let file = file_from_value(serde_json::from_str(&body)?)?;
    wait_until_active(api_key, file).await
}

async fn wait_until_active(
    api_key: &str,
    mut file: GeminiFile,
) -> Result<GeminiFile, Box<dyn Error>> {
    let http = reqwest::Client::new();
    let deadline = Instant::now() + Duration::from_secs(120);
    while file.state.as_deref() != Some("ACTIVE") {
        if Instant::now() > deadline {
            return Err(format!("Gemini file did not become ACTIVE: {file:?}").into());
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/{}",
            file.name
        );
        let response = http
            .get(url)
            .header("x-goog-api-key", api_key)
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(format!("Gemini Files get failed: status={status} body={body}").into());
        }
        file = file_from_value(serde_json::from_str(&body)?)?;
    }
    Ok(file)
}

async fn delete_file(api_key: &str, name: &str) {
    let url = format!("https://generativelanguage.googleapis.com/v1beta/{name}");
    let _ = reqwest::Client::new()
        .delete(url)
        .header("x-goog-api-key", api_key)
        .send()
        .await;
}
