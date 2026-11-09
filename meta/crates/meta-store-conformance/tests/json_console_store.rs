//! Exercise the public frame serde contract with a JSON-backed custom store.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use meerkat_meta::console_aggregator::{
    AppendDisposition, AppendOutcome, ConsoleCursor, ConsoleFrame, ConsoleFrameSourceKind,
    ConsoleFrameStatus, ConsoleLogResult, ConsoleLogStore, ConsoleTimelineMode,
    ConsoleTimelinePage, ConsoleTimelineQuery, ConsoleTimelineWindowPage,
    ConsoleTimelineWindowQuery, NewConsoleFrame,
};
use meta_store_conformance::{ConformanceFailure, ConsoleLogStoreFactory, chapters};
use serde_json::{Value, json};

#[derive(Default)]
struct JsonStorage {
    frames: Vec<Value>,
    watermarks: BTreeMap<(String, String), String>,
}

struct JsonStore {
    storage: Arc<Mutex<JsonStorage>>,
    discard_provenance: bool,
}

struct JsonFactory {
    storage: Arc<Mutex<JsonStorage>>,
    discard_provenance: bool,
}

#[async_trait]
impl ConsoleLogStoreFactory for JsonFactory {
    async fn open(&self) -> Result<Arc<dyn ConsoleLogStore>, ConformanceFailure> {
        // Each handle decodes persisted JSON, with no shared typed-frame cache.
        Ok(Arc::new(JsonStore {
            storage: self.storage.clone(),
            discard_provenance: self.discard_provenance,
        }))
    }
}

fn sequence(cursor: &ConsoleCursor) -> u64 {
    cursor
        .as_str()
        .strip_prefix("console:")
        .unwrap()
        .parse()
        .unwrap()
}

fn read_frames(storage: &JsonStorage) -> ConsoleLogResult<Vec<ConsoleFrame>> {
    storage
        .frames
        .iter()
        .cloned()
        .map(|value| Ok(serde_json::from_value(value)?))
        .collect()
}

#[async_trait]
impl ConsoleLogStore for JsonStore {
    async fn append_if_absent(&self, frame: NewConsoleFrame) -> ConsoleLogResult<AppendOutcome> {
        let mut storage = self.storage.lock().unwrap();
        if let Some(stored) = read_frames(&storage)?
            .into_iter()
            .find(|old| old.dedupe_key == frame.dedupe_key)
        {
            return Ok(AppendOutcome {
                disposition: AppendDisposition::Existing,
                frame: stored,
            });
        }
        let sequence = storage.frames.len() + 1;
        let id = frame
            .id
            .clone()
            .unwrap_or_else(|| format!("json-{sequence}"));
        let mut value = serde_json::to_value(frame)?;
        value["id"] = json!(id);
        value["cursor"] = json!(format!("console:{sequence}"));
        value["frame_version"] = json!(1);
        if self.discard_provenance {
            value["source"]
                .as_object_mut()
                .unwrap()
                .remove("member_provenance");
        }
        let stored = serde_json::from_value(value.clone())?;
        storage.frames.push(value);
        Ok(AppendOutcome {
            disposition: AppendDisposition::Inserted,
            frame: stored,
        })
    }

    async fn update_frame_status(
        &self,
        frame_id: &str,
        status: ConsoleFrameStatus,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        let mut storage = self.storage.lock().unwrap();
        let Some(value) = storage
            .frames
            .iter_mut()
            .find(|value| value["id"] == frame_id)
        else {
            return Ok(None);
        };
        let mut frame: ConsoleFrame = serde_json::from_value(value.clone())?;
        frame.status = status;
        frame.frame_version += 1;
        frame.updated_at_ms = Some(frame.timestamp_ms + frame.frame_version);
        *value = serde_json::to_value(&frame)?;
        Ok(Some(frame))
    }

    async fn query_frames(
        &self,
        query: ConsoleTimelineQuery,
    ) -> ConsoleLogResult<ConsoleTimelinePage> {
        let page = self.query_windowed_frames(query.into()).await?;
        Ok(ConsoleTimelinePage {
            frames: page.frames,
            next_cursor: page.next_cursor,
        })
    }

    async fn query_windowed_frames(
        &self,
        query: ConsoleTimelineWindowQuery,
    ) -> ConsoleLogResult<ConsoleTimelineWindowPage> {
        let storage = self.storage.lock().unwrap();
        let all = read_frames(&storage)?;
        let latest_cursor = all.last().map(|frame| frame.cursor.clone());
        let mut frames: Vec<_> = all
            .into_iter()
            .filter(|frame| {
                query
                    .identity
                    .as_ref()
                    .is_none_or(|identity| identity == &frame.identity)
                    && query
                        .conversation_id
                        .as_ref()
                        .is_none_or(|id| frame.conversation_id.as_ref() == Some(id))
                    && query
                        .after
                        .as_ref()
                        .is_none_or(|after| sequence(&frame.cursor) > sequence(after))
                    && query
                        .before
                        .as_ref()
                        .is_none_or(|before| sequence(&frame.cursor) < sequence(before))
            })
            .collect();
        let exhausted = frames.len() <= query.limit;
        if query.mode == ConsoleTimelineMode::Recent && frames.len() > query.limit {
            frames = frames.split_off(frames.len() - query.limit);
        } else {
            frames.truncate(query.limit);
        }
        Ok(ConsoleTimelineWindowPage {
            next_cursor: frames.last().map(|frame| frame.cursor.clone()),
            frames,
            latest_cursor,
            exhausted,
        })
    }

    async fn frame_by_dedupe_key(&self, key: &str) -> ConsoleLogResult<Option<ConsoleFrame>> {
        Ok(read_frames(&self.storage.lock().unwrap())?
            .into_iter()
            .find(|frame| frame.dedupe_key == key))
    }

    async fn latest_cursor(&self) -> ConsoleLogResult<Option<ConsoleCursor>> {
        Ok(read_frames(&self.storage.lock().unwrap())?
            .last()
            .map(|frame| frame.cursor.clone()))
    }

    async fn clear_frames(&self) -> ConsoleLogResult<()> {
        self.storage.lock().unwrap().frames.clear();
        Ok(())
    }

    async fn record_source_watermark(
        &self,
        runtime: &str,
        kind: ConsoleFrameSourceKind,
        cursor: &str,
    ) -> ConsoleLogResult<()> {
        self.storage.lock().unwrap().watermarks.insert(
            (runtime.to_string(), format!("{kind:?}")),
            cursor.to_string(),
        );
        Ok(())
    }

    async fn source_watermark(
        &self,
        runtime: &str,
        kind: ConsoleFrameSourceKind,
    ) -> ConsoleLogResult<Option<String>> {
        Ok(self
            .storage
            .lock()
            .unwrap()
            .watermarks
            .get(&(runtime.to_string(), format!("{kind:?}")))
            .cloned())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn json_console_store_preserves_provenance_across_handle_reopen() {
    let factory = JsonFactory {
        storage: Arc::default(),
        discard_provenance: false,
    };
    chapters::console_log(&factory)
        .await
        .expect("serde-backed custom storage preserves policy provenance");
}

#[tokio::test]
async fn console_conformance_rejects_store_that_drops_member_provenance() {
    let factory = JsonFactory {
        storage: Arc::default(),
        discard_provenance: true,
    };
    let failure = chapters::console_log(&factory)
        .await
        .expect_err("dropping policy provenance must fail conformance");
    assert_eq!(failure.chapter(), "console_log");
    assert_eq!(failure.step(), "member_provenance_roundtrip");
}
