//! Shared voice-fixture mechanics for the live audio lanes.
//!
//! Two consumers share this module: the `smoke_shared_realm` live-adapter
//! smokes (which synthesize and stream PCM through the WebSocket live
//! adapter) and the `voice_fixtures` bin (which mints and verifies the
//! browser-peer WAV fixtures under
//! `tests/live_smoke/browser/fixtures/gpt_live_client`). Both must apply the
//! same normalisation so a fixture minted here behaves like a synthesized
//! smoke utterance: internal pauses are compressed (server VAD must not
//! split one utterance into two) and a trailing-silence floor is appended
//! (server VAD needs it to emit `speech_stopped` on long utterances).

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const OPENAI_TTS_MODEL: &str = "gpt-4o-mini-tts";
pub const OPENAI_TTS_DEFAULT_VOICE: &str = "alloy";
pub const LIVE_AUDIO_SAMPLE_RATE_HZ: usize = 24_000;
pub const LIVE_AUDIO_BYTES_PER_SAMPLE: usize = 2;
pub const LIVE_AUDIO_FRAME_MS: usize = 200;
/// 1500 ms (not 500 ms): gpt-realtime-2 server VAD requires this floor for
/// reliable `speech_stopped` on long utterances. See commit f08ce9e3b.
pub const LIVE_AUDIO_TRAILING_SILENCE_MS: usize = 1500;
pub const LIVE_AUDIO_INTERNAL_SILENCE_THRESHOLD: i16 = 100;
pub const LIVE_AUDIO_MAX_INTERNAL_SILENCE_MS: usize = 200;
pub const LIVE_AUDIO_PRESERVED_INTERNAL_SILENCE_MS: usize = 80;

/// Repository-relative directory of the browser-peer WAV fixtures.
pub const FIXTURE_DIR: &str = "tests/live_smoke/browser/fixtures/gpt_live_client";
pub const MANIFEST_FILE: &str = "manifest.json";
/// A verified fixture's decoded duration may differ from the manifest by at
/// most this much (covers WAV chunk padding and header rounding, not a
/// re-mint).
pub const DURATION_TOLERANCE_MS: u32 = 50;

#[derive(Debug)]
pub enum VoiceFixtureError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    Http(reqwest::Error),
    Tts {
        status: u16,
        body: String,
    },
    Wav {
        path: PathBuf,
        reason: String,
    },
    UnknownFixture(String),
    Verification(Vec<String>),
}

impl fmt::Display for VoiceFixtureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Json { path, source } => write!(f, "{}: invalid JSON: {source}", path.display()),
            Self::Http(error) => write!(f, "OpenAI TTS request failed: {error}"),
            Self::Tts { status, body } => {
                write!(f, "OpenAI TTS request failed with HTTP {status}: {body}")
            }
            Self::Wav { path, reason } => write!(f, "{}: {reason}", path.display()),
            Self::UnknownFixture(name) => write!(f, "manifest has no fixture named {name:?}"),
            Self::Verification(problems) => {
                writeln!(f, "{} fixture verification problem(s):", problems.len())?;
                for problem in problems {
                    writeln!(f, "  - {problem}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for VoiceFixtureError {}

impl From<reqwest::Error> for VoiceFixtureError {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error)
    }
}

// ---------------------------------------------------------------------------
// PCM normalisation (byte-identical to the former smoke_shared_realm helpers)
// ---------------------------------------------------------------------------

pub fn live_pcm_bytes_per_ms() -> usize {
    (LIVE_AUDIO_SAMPLE_RATE_HZ * LIVE_AUDIO_BYTES_PER_SAMPLE) / 1000
}

pub fn append_pcm_trailing_silence(pcm: &[u8], trailing_silence_ms: usize) -> Vec<u8> {
    let silence_bytes = live_pcm_bytes_per_ms() * trailing_silence_ms;
    let mut output = Vec::with_capacity(pcm.len() + silence_bytes);
    output.extend_from_slice(pcm);
    output.resize(output.len() + silence_bytes, 0);
    output
}

pub fn compress_internal_pcm_silence(
    pcm: &[u8],
    amplitude_threshold: i16,
    max_silence_ms: usize,
    preserved_silence_ms: usize,
) -> Vec<u8> {
    let max_silence_bytes = live_pcm_bytes_per_ms() * max_silence_ms;
    let preserved_silence_bytes = live_pcm_bytes_per_ms() * preserved_silence_ms;
    let mut output = Vec::with_capacity(pcm.len());
    let mut index = 0usize;

    while index + LIVE_AUDIO_BYTES_PER_SAMPLE <= pcm.len() {
        let sample = i16::from_le_bytes([pcm[index], pcm[index + 1]]);
        let silent = sample.abs() <= amplitude_threshold;
        let run_start = index;
        index += LIVE_AUDIO_BYTES_PER_SAMPLE;

        while index + LIVE_AUDIO_BYTES_PER_SAMPLE <= pcm.len() {
            let next_sample = i16::from_le_bytes([pcm[index], pcm[index + 1]]);
            if (next_sample.abs() <= amplitude_threshold) != silent {
                break;
            }
            index += LIVE_AUDIO_BYTES_PER_SAMPLE;
        }

        let run = &pcm[run_start..index];
        if silent && run.len() > max_silence_bytes {
            output.extend_from_slice(&run[..preserved_silence_bytes.min(run.len())]);
        } else {
            output.extend_from_slice(run);
        }
    }

    if index < pcm.len() {
        output.extend_from_slice(&pcm[index..]);
    }

    output
}

pub fn prepare_tts_pcm_for_live_vad(pcm: &[u8]) -> Vec<u8> {
    compress_internal_pcm_silence(
        pcm,
        LIVE_AUDIO_INTERNAL_SILENCE_THRESHOLD,
        LIVE_AUDIO_MAX_INTERNAL_SILENCE_MS,
        LIVE_AUDIO_PRESERVED_INTERNAL_SILENCE_MS,
    )
}

pub fn chunk_pcm_bytes(pcm: &[u8], frame_ms: usize, trailing_silence_ms: usize) -> Vec<Vec<u8>> {
    let frame_bytes = live_pcm_bytes_per_ms() * frame_ms;
    append_pcm_trailing_silence(pcm, trailing_silence_ms)
        .chunks(frame_bytes.max(1))
        .map(|chunk| chunk.to_vec())
        .collect()
}

pub fn pcm_has_non_silence(pcm: &[u8]) -> bool {
    pcm.chunks_exact(2)
        .map(|sample| i16::from_le_bytes([sample[0], sample[1]]))
        .any(|sample| sample != 0)
}

/// Milliseconds of trailing samples at or below the internal-silence
/// amplitude threshold.
pub fn trailing_silence_ms(pcm: &[u8]) -> u32 {
    let trailing = pcm
        .chunks_exact(2)
        .rev()
        .take_while(|sample| {
            i16::from_le_bytes([sample[0], sample[1]]).abs()
                <= LIVE_AUDIO_INTERNAL_SILENCE_THRESHOLD
        })
        .count();
    u32::try_from(trailing * 1000 / LIVE_AUDIO_SAMPLE_RATE_HZ).unwrap_or(u32::MAX)
}

pub fn pcm_duration_ms(pcm: &[u8]) -> u32 {
    u32::try_from((pcm.len() / LIVE_AUDIO_BYTES_PER_SAMPLE) * 1000 / LIVE_AUDIO_SAMPLE_RATE_HZ)
        .unwrap_or(u32::MAX)
}

pub fn live_audio_cache_key(text: &str, model: &str, voice: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"openai-tts-v2\0");
    digest.update(model.as_bytes());
    digest.update(b"\0");
    digest.update(voice.as_bytes());
    digest.update(b"\0");
    digest.update(text.as_bytes());
    format!("{:x}", digest.finalize())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

// ---------------------------------------------------------------------------
// OpenAI speech synthesis
// ---------------------------------------------------------------------------

/// Raw 24 kHz PCM16 mono speech from the OpenAI speech API. No cache, no
/// normalisation: callers decide how to prepare it for their transport.
pub async fn synthesize_openai_tts_pcm(
    api_key: &str,
    model: &str,
    voice: &str,
    text: &str,
) -> Result<Vec<u8>, VoiceFixtureError> {
    let response = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(60))
        .build()?
        .post("https://api.openai.com/v1/audio/speech")
        .bearer_auth(api_key)
        .json(&serde_json::json!({
            "model": model,
            "voice": voice,
            "input": text,
            "response_format": "pcm",
        }))
        .send()
        .await?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        return Err(VoiceFixtureError::Tts { status, body });
    }
    Ok(response.bytes().await?.to_vec())
}

/// VAD-prepared speech, cached under `cache_dir` by (model, voice, text).
/// This is the live-adapter smoke path: the trailing-silence floor is
/// appended by the streaming chunker, not stored in the cache.
pub async fn cached_openai_tts_pcm(
    api_key: &str,
    cache_dir: &Path,
    model: &str,
    voice: &str,
    text: &str,
) -> Result<Vec<u8>, VoiceFixtureError> {
    let cache_path = cache_dir.join(format!("{}.pcm", live_audio_cache_key(text, model, voice)));
    if cache_path.exists() {
        return tokio::fs::read(&cache_path)
            .await
            .map_err(|source| VoiceFixtureError::Io {
                path: cache_path,
                source,
            });
    }
    tokio::fs::create_dir_all(cache_dir)
        .await
        .map_err(|source| VoiceFixtureError::Io {
            path: cache_dir.to_path_buf(),
            source,
        })?;
    let pcm = prepare_tts_pcm_for_live_vad(
        &synthesize_openai_tts_pcm(api_key, model, voice, text).await?,
    );
    tokio::fs::write(&cache_path, &pcm)
        .await
        .map_err(|source| VoiceFixtureError::Io {
            path: cache_path,
            source,
        })?;
    Ok(pcm)
}

// ---------------------------------------------------------------------------
// WAV (RIFF) PCM16 mono codec
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WavPcm16Mono {
    pub sample_rate_hz: u32,
    pub pcm: Vec<u8>,
}

impl WavPcm16Mono {
    pub fn duration_ms(&self) -> u32 {
        u32::try_from(
            (self.pcm.len() / LIVE_AUDIO_BYTES_PER_SAMPLE) as u64 * 1000
                / u64::from(self.sample_rate_hz.max(1)),
        )
        .unwrap_or(u32::MAX)
    }
}

/// Decode a RIFF/WAVE file, walking chunks (macOS `say` emits a `FLLR`
/// filler chunk, ffmpeg a `LIST` chunk) and requiring PCM16 mono.
pub fn decode_wav(path: &Path, bytes: &[u8]) -> Result<WavPcm16Mono, VoiceFixtureError> {
    let wav = |reason: String| VoiceFixtureError::Wav {
        path: path.to_path_buf(),
        reason,
    };
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(wav("not a RIFF/WAVE file".into()));
    }
    let mut position = 12usize;
    let mut format = None;
    let mut data = None;
    while position + 8 <= bytes.len() {
        let id = &bytes[position..position + 4];
        let size = u32::from_le_bytes([
            bytes[position + 4],
            bytes[position + 5],
            bytes[position + 6],
            bytes[position + 7],
        ]) as usize;
        let body_start = position + 8;
        let body_end = body_start
            .checked_add(size)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| {
                wav(format!(
                    "chunk {} overruns the file",
                    String::from_utf8_lossy(id)
                ))
            })?;
        let body = &bytes[body_start..body_end];
        match id {
            b"fmt " => {
                if body.len() < 16 {
                    return Err(wav("fmt chunk shorter than 16 bytes".into()));
                }
                let audio_format = u16::from_le_bytes([body[0], body[1]]);
                let channels = u16::from_le_bytes([body[2], body[3]]);
                let sample_rate = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
                let bits = u16::from_le_bytes([body[14], body[15]]);
                if audio_format != 1 {
                    return Err(wav(format!("audio format {audio_format} is not PCM (1)")));
                }
                if channels != 1 {
                    return Err(wav(format!("{channels} channels; expected mono")));
                }
                if bits != 16 {
                    return Err(wav(format!("{bits} bits per sample; expected 16")));
                }
                format = Some(sample_rate);
            }
            b"data" => data = Some(body.to_vec()),
            _ => {}
        }
        position = body_end + (size & 1);
    }
    let sample_rate_hz = format.ok_or_else(|| wav("missing fmt chunk".into()))?;
    let pcm = data.ok_or_else(|| wav("missing data chunk".into()))?;
    Ok(WavPcm16Mono {
        sample_rate_hz,
        pcm,
    })
}

pub fn encode_wav(sample_rate_hz: u32, pcm: &[u8]) -> Vec<u8> {
    let data_len = u32::try_from(pcm.len()).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&sample_rate_hz.to_le_bytes());
    out.extend_from_slice(&(sample_rate_hz * LIVE_AUDIO_BYTES_PER_SAMPLE as u32).to_le_bytes());
    out.extend_from_slice(&(LIVE_AUDIO_BYTES_PER_SAMPLE as u16).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(pcm);
    out
}

// ---------------------------------------------------------------------------
// Fixture manifest
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureManifest {
    /// Every fixture must decode at this rate; the browser peer's
    /// AudioContext runs at it.
    pub sample_rate_hz: u32,
    /// Trailing-silence floor appended when minting.
    pub mint_trailing_silence_ms: u32,
    pub fixtures: Vec<FixtureEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureEntry {
    /// Harness fixture name (`play {name}`).
    pub name: String,
    /// WAV file name relative to the fixture directory.
    pub file: String,
    /// TTS voice. OpenAI voice ids re-mint byte-similar audio; historical
    /// `macos-say-samantha` / `unknown` fixtures cannot be re-minted.
    pub voice: String,
    /// Exact spoken script.
    pub text: String,
    /// When non-empty, the script is minted segment by segment and the
    /// segments are joined with `pauses_ms` of digital silence, so a
    /// deliberate mid-utterance pause survives the internal-silence
    /// compression (which is applied within each segment only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<String>,
    /// Silence between consecutive segments; `pauses_ms.len()` must be
    /// `segments.len() - 1`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pauses_ms: Vec<u32>,
    /// SHA-256 of the committed WAV bytes.
    pub sha256: String,
    /// Decoded PCM duration.
    pub duration_ms: u32,
    pub sample_rate_hz: u32,
    /// Trailing samples at or below the internal-silence threshold.
    pub trailing_silence_ms: u32,
    pub notes: String,
}

impl FixtureEntry {
    pub fn is_mintable(&self) -> bool {
        !self.voice.starts_with("macos-") && self.voice != "unknown"
    }
}

impl FixtureManifest {
    pub fn load(path: &Path) -> Result<Self, VoiceFixtureError> {
        let bytes = std::fs::read(path).map_err(|source| VoiceFixtureError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        serde_json::from_slice(&bytes).map_err(|source| VoiceFixtureError::Json {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn save(&self, path: &Path) -> Result<(), VoiceFixtureError> {
        let mut json =
            serde_json::to_vec_pretty(self).map_err(|source| VoiceFixtureError::Json {
                path: path.to_path_buf(),
                source,
            })?;
        json.push(b'\n');
        std::fs::write(path, json).map_err(|source| VoiceFixtureError::Io {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn entry(&self, name: &str) -> Result<&FixtureEntry, VoiceFixtureError> {
        self.fixtures
            .iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| VoiceFixtureError::UnknownFixture(name.to_owned()))
    }

    pub fn entry_mut(&mut self, name: &str) -> Result<&mut FixtureEntry, VoiceFixtureError> {
        self.fixtures
            .iter_mut()
            .find(|entry| entry.name == name)
            .ok_or_else(|| VoiceFixtureError::UnknownFixture(name.to_owned()))
    }

    /// Check every entry against the WAV on disk. All problems are collected
    /// so one run reports every drifted fixture.
    pub fn verify(&self, fixture_dir: &Path) -> Result<Vec<VerifiedFixture>, VoiceFixtureError> {
        let mut problems = Vec::new();
        let mut verified = Vec::new();
        let mut names = std::collections::BTreeSet::new();
        for entry in &self.fixtures {
            if !names.insert(entry.name.as_str()) {
                problems.push(format!("{}: duplicate fixture name", entry.name));
            }
            let path = fixture_dir.join(&entry.file);
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    problems.push(format!("{}: {}: {error}", entry.name, path.display()));
                    continue;
                }
            };
            let sha256 = sha256_hex(&bytes);
            if sha256 != entry.sha256 {
                problems.push(format!(
                    "{}: sha256 {sha256} != manifest {}",
                    entry.name, entry.sha256
                ));
            }
            let wav = match decode_wav(&path, &bytes) {
                Ok(wav) => wav,
                Err(error) => {
                    problems.push(format!("{}: {error}", entry.name));
                    continue;
                }
            };
            if wav.sample_rate_hz != self.sample_rate_hz
                || entry.sample_rate_hz != self.sample_rate_hz
            {
                problems.push(format!(
                    "{}: sample rate wav={} manifest={} required={}",
                    entry.name, wav.sample_rate_hz, entry.sample_rate_hz, self.sample_rate_hz
                ));
            }
            let duration_ms = wav.duration_ms();
            if duration_ms.abs_diff(entry.duration_ms) > DURATION_TOLERANCE_MS {
                problems.push(format!(
                    "{}: duration {duration_ms} ms != manifest {} ms (tolerance {DURATION_TOLERANCE_MS} ms)",
                    entry.name, entry.duration_ms
                ));
            }
            let trailing = trailing_silence_ms(&wav.pcm);
            if trailing.abs_diff(entry.trailing_silence_ms) > DURATION_TOLERANCE_MS {
                problems.push(format!(
                    "{}: trailing silence {trailing} ms != manifest {} ms (tolerance {DURATION_TOLERANCE_MS} ms)",
                    entry.name, entry.trailing_silence_ms
                ));
            }
            if !pcm_has_non_silence(&wav.pcm) {
                problems.push(format!("{}: WAV is digital silence", entry.name));
            }
            if entry.text.trim().is_empty() {
                problems.push(format!("{}: empty text", entry.name));
            }
            verified.push(VerifiedFixture {
                name: entry.name.clone(),
                duration_ms,
                trailing_silence_ms: trailing,
            });
        }
        if problems.is_empty() {
            Ok(verified)
        } else {
            Err(VoiceFixtureError::Verification(problems))
        }
    }
}

#[derive(Clone, Debug)]
pub struct VerifiedFixture {
    pub name: String,
    pub duration_ms: u32,
    pub trailing_silence_ms: u32,
}

/// Synthesize `entry` with the manifest's voice and text, normalise it like
/// a live-adapter smoke utterance, append the trailing-silence floor, write
/// the WAV, and refresh the entry's digest, duration, and silence fields.
pub async fn mint_fixture(
    api_key: &str,
    fixture_dir: &Path,
    manifest_sample_rate_hz: u32,
    trailing_silence_ms_floor: u32,
    entry: &mut FixtureEntry,
) -> Result<WavPcm16Mono, VoiceFixtureError> {
    let prepared = if entry.segments.is_empty() {
        let raw =
            synthesize_openai_tts_pcm(api_key, OPENAI_TTS_MODEL, &entry.voice, &entry.text).await?;
        prepare_tts_pcm_for_live_vad(&raw)
    } else {
        if entry.pauses_ms.len() + 1 != entry.segments.len() {
            return Err(VoiceFixtureError::Wav {
                path: fixture_dir.join(&entry.file),
                reason: format!(
                    "{} segments need {} pauses, manifest has {}",
                    entry.segments.len(),
                    entry.segments.len() - 1,
                    entry.pauses_ms.len()
                ),
            });
        }
        let mut joined = Vec::new();
        for (index, segment) in entry.segments.iter().enumerate() {
            if index > 0 {
                joined = append_pcm_trailing_silence(&joined, entry.pauses_ms[index - 1] as usize);
            }
            let raw =
                synthesize_openai_tts_pcm(api_key, OPENAI_TTS_MODEL, &entry.voice, segment).await?;
            joined.extend_from_slice(&prepare_tts_pcm_for_live_vad(&raw));
        }
        joined
    };
    let pcm = append_pcm_trailing_silence(&prepared, trailing_silence_ms_floor as usize);
    let bytes = encode_wav(manifest_sample_rate_hz, &pcm);
    let path = fixture_dir.join(&entry.file);
    std::fs::write(&path, &bytes).map_err(|source| VoiceFixtureError::Io {
        path: path.clone(),
        source,
    })?;
    let wav = WavPcm16Mono {
        sample_rate_hz: manifest_sample_rate_hz,
        pcm,
    };
    entry.sha256 = sha256_hex(&bytes);
    entry.duration_ms = wav.duration_ms();
    entry.sample_rate_hz = manifest_sample_rate_hz;
    entry.trailing_silence_ms = trailing_silence_ms(&wav.pcm);
    Ok(wav)
}

/// Workspace root: `MEERKAT_WORKSPACE_ROOT`, else the nearest ancestor of
/// the current directory (falling back to this crate's manifest directory)
/// that holds the browser fixture tree.
pub fn workspace_root() -> Option<PathBuf> {
    if let Some(root) = std::env::var_os("MEERKAT_WORKSPACE_ROOT") {
        return Some(PathBuf::from(root));
    }
    let is_root = |candidate: &Path| {
        candidate.join("Cargo.toml").is_file() && candidate.join(FIXTURE_DIR).is_dir()
    };
    std::env::current_dir()
        .ok()
        .and_then(|dir| dir.ancestors().find(|c| is_root(c)).map(Path::to_path_buf))
        .or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .ancestors()
                .find(|c| is_root(c))
                .map(Path::to_path_buf)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn committed_manifest_matches_the_committed_fixtures() {
        let root = workspace_root().expect("workspace root");
        let dir = root.join(FIXTURE_DIR);
        let manifest = FixtureManifest::load(&dir.join(MANIFEST_FILE)).unwrap();
        let verified = manifest
            .verify(&dir)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(verified.len(), manifest.fixtures.len());
        // Every WAV in the directory is declared.
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|ext| ext == "wav") {
                let file = path.file_name().unwrap().to_string_lossy().into_owned();
                assert!(
                    manifest.fixtures.iter().any(|f| f.file == file),
                    "{file} is not declared in {MANIFEST_FILE}"
                );
            }
        }
    }

    #[test]
    fn wav_round_trip_preserves_pcm_and_walks_filler_chunks() {
        let pcm: Vec<u8> = (0..4800u16)
            .flat_map(|i| (i as i16).to_le_bytes())
            .collect();
        let bytes = encode_wav(24_000, &pcm);
        let wav = decode_wav(Path::new("x.wav"), &bytes).unwrap();
        assert_eq!(wav.sample_rate_hz, 24_000);
        assert_eq!(wav.pcm, pcm);
        assert_eq!(wav.duration_ms(), 200);
        // Insert an odd-sized FLLR chunk before data, as macOS `say` does.
        let mut padded = bytes[..36].to_vec();
        padded.extend_from_slice(b"FLLR");
        padded.extend_from_slice(&3u32.to_le_bytes());
        padded.extend_from_slice(&[0, 0, 0, 0]);
        padded.extend_from_slice(&bytes[36..]);
        assert_eq!(decode_wav(Path::new("x.wav"), &padded).unwrap().pcm, pcm);
        let mut stereo = bytes.clone();
        stereo[22] = 2;
        assert!(decode_wav(Path::new("x.wav"), &stereo).is_err());
    }

    #[test]
    fn minted_pcm_gets_the_trailing_silence_floor_and_compressed_pauses() {
        let bytes_per_ms = live_pcm_bytes_per_ms();
        let mut pcm = vec![1u8, 4]
            .into_iter()
            .cycle()
            .take(bytes_per_ms * 100)
            .collect::<Vec<_>>();
        pcm.extend(std::iter::repeat_n(0u8, bytes_per_ms * 600));
        pcm.extend(vec![1u8, 4].into_iter().cycle().take(bytes_per_ms * 100));
        let prepared = append_pcm_trailing_silence(
            &prepare_tts_pcm_for_live_vad(&pcm),
            LIVE_AUDIO_TRAILING_SILENCE_MS,
        );
        assert_eq!(
            pcm_duration_ms(&prepared) as usize,
            100 + LIVE_AUDIO_PRESERVED_INTERNAL_SILENCE_MS + 100 + LIVE_AUDIO_TRAILING_SILENCE_MS
        );
        assert_eq!(
            trailing_silence_ms(&prepared) as usize,
            LIVE_AUDIO_TRAILING_SILENCE_MS
        );
    }
}
