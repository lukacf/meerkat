//! The blob half of the continuity mirror: which ids a session document
//! references, how the file sink keeps blobs, the order a round puts them
//! in, and the mirror-to-restore round trip through the golden files in
//! `tests/fixtures/continuity_mirror_blobs_v1` (the restore script's suite
//! checks that it writes exactly the restored tree kept there).

use std::collections::BTreeMap;
use std::io::Write as _;
use std::sync::Mutex;

use bytes::Bytes;
use meerkat_core::types::{ContentBlock, ImageData, Message, UserMessage};
use meerkat_core::{BlobAddressAttestation, BlobStore as _, MissingBlobBehavior, Session};

use super::*;
use crate::blob_store::{Base64BlobStoreAdapter, ObjectStoreBlobStore};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nsynthetic continuity mirror image";
const RAW: &[u8] = b"\xff\xd8\xff\xe0synthetic blob under the raw address";
const PNG_ID: &str = "sha256:d4fd90516c29aeb0c46882202b1bafd677104f13678facd90830881b740d466f";
const RAW_ID: &str = "sha256:1c38628b50abcc0425f74b5a174409180a486d5808f5ade45bd0931522a0347f";

const GOLDEN_BLOBS: &str =
    include_str!("../../tests/fixtures/continuity_mirror_blobs_v1/blobs.jsonl");

/// The tree `restore_blobs.py` writes for [`GOLDEN_BLOBS`], below `blobs/`.
const RESTORED: &[(&str, &[u8])] = &[
    (
        "meta/1c38628b50abcc0425f74b5a174409180a486d5808f5ade45bd0931522a0347f.json",
        include_bytes!(
            "../../tests/fixtures/continuity_mirror_blobs_v1/restored/blobs/meta/1c38628b50abcc0425f74b5a174409180a486d5808f5ade45bd0931522a0347f.json"
        ),
    ),
    (
        "meta/d4fd90516c29aeb0c46882202b1bafd677104f13678facd90830881b740d466f.json",
        include_bytes!(
            "../../tests/fixtures/continuity_mirror_blobs_v1/restored/blobs/meta/d4fd90516c29aeb0c46882202b1bafd677104f13678facd90830881b740d466f.json"
        ),
    ),
    (
        "objects/1c38628b50abcc0425f74b5a174409180a486d5808f5ade45bd0931522a0347f.bin",
        include_bytes!(
            "../../tests/fixtures/continuity_mirror_blobs_v1/restored/blobs/objects/1c38628b50abcc0425f74b5a174409180a486d5808f5ade45bd0931522a0347f.bin"
        ),
    ),
    (
        "objects/d4fd90516c29aeb0c46882202b1bafd677104f13678facd90830881b740d466f.bin",
        include_bytes!(
            "../../tests/fixtures/continuity_mirror_blobs_v1/restored/blobs/objects/d4fd90516c29aeb0c46882202b1bafd677104f13678facd90830881b740d466f.bin"
        ),
    ),
];

fn id(fill: char) -> String {
    format!("sha256:{}", fill.to_string().repeat(64))
}

fn row(fill: char) -> ContinuityMirrorBlob {
    ContinuityMirrorBlob {
        blob_id: id(fill),
        media_type: "image/png".to_string(),
        data: "AA==".to_string(),
    }
}

fn rows(text: &str) -> Result<Vec<ContinuityMirrorBlob>, serde_json::Error> {
    text.lines().map(serde_json::from_str).collect()
}

/// A blob store over the tree the restore writes for [`GOLDEN_BLOBS`] in a
/// fresh state directory, and the directory, which must outlive the store.
fn restored_store()
-> Result<(tempfile::TempDir, Base64BlobStoreAdapter), Box<dyn std::error::Error>> {
    let state = tempfile::tempdir()?;
    let blobs = state.path().join("blobs");
    for (relative, content) in RESTORED {
        let path = blobs.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)?;
    }
    let store: Arc<dyn BinaryBlobStore> = Arc::new(ObjectStoreBlobStore::local(blobs)?);
    Ok((state, Base64BlobStoreAdapter::new(store)))
}

/// The inline image data in the user rows of `messages`.
fn inline_images(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::User(user) => Some(&user.content),
            _ => None,
        })
        .flatten()
        .filter_map(|block| match block {
            ContentBlock::Image {
                data: ImageData::Inline { data },
                ..
            } => Some(data.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn collects_every_blob_id_a_session_document_references() -> TestResult {
    let image = serde_json::to_value(ContentBlock::Image {
        media_type: "image/png".to_string(),
        data: ImageData::Blob {
            blob_id: BlobId::new(id('a')),
        },
    })?;
    let document = serde_json::json!({
        "messages": [{
            "role": "user",
            "content": [
                image,
                {"type": "image", "media_type": "image/png", "source": "inline", "data": "iVBORw0KGgo="},
                {"type": "text", "text": format!("{{\"blob_id\":\"{}\"}}", id('d'))},
            ],
        }],
        "metadata": {
            "history": {"bodies": [[{"type": "image", "source": "blob", "blob_id": id('b')}]]},
            "realtime": {"blob_id": id('c'), "media_type": "image/png"},
            "not_canonical": {"blob_id": "sha256:ABC"},
            "not_a_string": {"blob_id": {"blob_id": id('e')}},
        },
    });
    let mut ids = BTreeSet::new();
    collect_blob_ids(&document, &mut ids);
    assert_eq!(ids, BTreeSet::from([id('a'), id('b'), id('c'), id('e')]));
    assert!(mentions_blob_id(&serde_json::to_vec(&document)?));
    // The check may let a document without blobs through (a string value
    // `blob_id` has the same bytes), which only costs a parse; it never
    // turns one that names a blob away.
    let no_blobs = serde_json::json!({"messages": [{"role": "user", "content": "a blob"}]});
    assert!(!mentions_blob_id(&serde_json::to_vec(&no_blobs)?));
    Ok(())
}

#[tokio::test]
async fn the_file_sink_appends_each_blob_once_and_cuts_a_torn_tail() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join(CONTINUITY_MIRROR_BLOBS_FILE);
    let sink = JsonlFileMirrorSink::new(dir.path());
    sink.put_blob(&row('a')).await?;
    sink.put_blob(&row('a')).await?;
    // A new sink over the same directory knows the ids the file holds.
    let restarted = JsonlFileMirrorSink::new(dir.path());
    restarted.put_blob(&row('a')).await?;
    restarted.put_blob(&row('b')).await?;
    assert_eq!(
        rows(&std::fs::read_to_string(&path)?)?,
        [row('a'), row('b')]
    );

    // An append a crash cut short leaves a line without its newline.
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)?
        .write_all(b"{\"blob_id\":\"sha256:cc")?;
    let after_crash = JsonlFileMirrorSink::new(dir.path());
    after_crash.put_blob(&row('c')).await?;
    let text = std::fs::read_to_string(&path)?;
    assert!(text.ends_with('\n'), "{text}");
    assert_eq!(rows(&text)?, [row('a'), row('b'), row('c')]);
    Ok(())
}

#[tokio::test]
async fn mirror_rows_read_from_a_real_store_are_the_golden_blobs_file() -> TestResult {
    let state = tempfile::tempdir()?;
    let store = Arc::new(ObjectStoreBlobStore::local(state.path().join("blobs"))?);
    // An image as a turn stores it (Meerkat's address over the base64 text),
    // and a blob under Meta's raw address.
    let image = Base64BlobStoreAdapter::new(store.clone())
        .put_image(
            "image/png",
            &base64::engine::general_purpose::STANDARD.encode(PNG),
        )
        .await?;
    let raw = store
        .put_bytes("image/jpeg", Bytes::from_static(RAW))
        .await?;
    assert_eq!(image.blob_id.to_string(), PNG_ID);
    assert_eq!(raw.blob_id.to_string(), RAW_ID);

    let mirror = tempfile::tempdir()?;
    let sink = JsonlFileMirrorSink::new(mirror.path());
    for blob_id in BTreeSet::from([PNG_ID, RAW_ID]) {
        let blob = read_mirror_blob(store.as_ref(), blob_id)
            .await?
            .ok_or("the store holds the blob")?;
        sink.put_blob(&blob).await?;
    }
    assert_eq!(
        std::fs::read_to_string(mirror.path().join(CONTINUITY_MIRROR_BLOBS_FILE))?,
        GOLDEN_BLOBS
    );
    Ok(())
}

#[tokio::test]
async fn a_blob_the_store_lacks_or_holds_under_another_address_is_not_copied() -> TestResult {
    let state = tempfile::tempdir()?;
    let store = ObjectStoreBlobStore::local(state.path().join("blobs"))?;
    assert_eq!(read_mirror_blob(&store, &id('e')).await?, None);
    store
        .put_bytes_addressed(BlobId::new(id('f')), "image/png", Bytes::from_static(PNG))
        .await?;
    assert_eq!(read_mirror_blob(&store, &id('f')).await?, None);
    Ok(())
}

/// R6: the images a mirror copied, written into a fresh state directory by
/// the restore, hydrate with their original bytes.
#[tokio::test]
async fn an_image_restored_from_the_mirror_hydrates_with_its_original_bytes() -> TestResult {
    let (_state, adapter) = restored_store()?;
    let golden = rows(GOLDEN_BLOBS)?;
    assert_eq!(golden.len(), 2);
    for blob in &golden {
        let mut blocks = vec![ContentBlock::Image {
            media_type: blob.media_type.clone(),
            data: ImageData::Blob {
                blob_id: BlobId::new(blob.blob_id.clone()),
            },
        }];
        meerkat_core::hydrate_content_blocks(&adapter, &mut blocks, MissingBlobBehavior::Error)
            .await?;
        match blocks.as_slice() {
            [
                ContentBlock::Image {
                    data: ImageData::Inline { data },
                    ..
                },
            ] => assert_eq!(data, &blob.data, "{}", blob.blob_id),
            other => return Err(format!("{}: {other:?}", blob.blob_id).into()),
        }
    }
    // The checks a turn's hydration runs: the image's id is Meerkat's content
    // address of the bytes it hydrates to, and the raw blob's id is the
    // store's own address of its payload.
    let image = adapter.get(&BlobId::new(PNG_ID)).await?;
    assert_eq!(
        meerkat_core::blob::content_blob_id(&image.media_type, &image.data).to_string(),
        PNG_ID
    );
    assert_eq!(
        image.data,
        base64::engine::general_purpose::STANDARD.encode(PNG)
    );
    let raw = adapter.get(&BlobId::new(RAW_ID)).await?;
    assert!(matches!(
        adapter.attest_address(&BlobId::new(RAW_ID), &raw).await?,
        BlobAddressAttestation::StoreAddress
    ));
    Ok(())
}

/// R6 with the image inside a compaction: once a compaction retires the
/// turn that holds the image, only the session's history names it. The
/// mirror still copies it, and after the restore the history renders it with
/// its bytes, not as a placeholder.
#[tokio::test]
async fn an_image_inside_a_compaction_is_mirrored_and_renders_after_the_restore() -> TestResult {
    let source = tempfile::tempdir()?;
    let store: Arc<dyn BinaryBlobStore> =
        Arc::new(ObjectStoreBlobStore::local(source.path().join("blobs"))?);
    let mut session = Session::new();
    session.append_system_message("system prompt");
    session.push(Message::User(UserMessage::text("question 0")));
    let mut look = UserMessage::text("look at this");
    look.content.push(ContentBlock::Image {
        media_type: "image/png".to_string(),
        data: ImageData::Inline {
            data: base64::engine::general_purpose::STANDARD.encode(PNG),
        },
    });
    session.push(Message::User(look));
    session.push(Message::User(UserMessage::text("and then")));
    // A checkpoint moves the image into the store, as a turn's does.
    session
        .externalize_media(&Base64BlobStoreAdapter::new(store.clone()), 0)
        .await?;
    let before = session.transcript_revision()?;
    let live = session.messages().to_vec();
    let _compaction = session.stage_validated_compaction_for_test(
        vec![
            live[0].clone(),
            Message::User(UserMessage::compaction_summary(
                "[Compaction summary] an image",
            )),
            live[3].clone(),
        ],
        12,
    )?;
    assert!(
        meerkat_core::image_content::collect_blob_ids_from_messages(session.messages()).is_empty(),
        "only the history names the image"
    );

    // The mirror finds the image in the session document and copies the
    // golden row, which the restore suite writes into the golden tree.
    let mut ids = BTreeSet::new();
    collect_document_blob_ids(&session.to_persisted_bytes()?, &mut ids)?;
    assert_eq!(ids, BTreeSet::from([PNG_ID.to_string()]));
    let blob = read_mirror_blob(store.as_ref(), PNG_ID)
        .await?
        .ok_or("the store holds the image")?;
    assert!(rows(GOLDEN_BLOBS)?.contains(&blob));

    let retained = session
        .retained_transcript_revision_rows(&before)?
        .ok_or("the session retains the compacted revision")?;
    let (_restored_state, restored) = restored_store()?;
    let mut rendered = retained.messages().to_vec();
    meerkat_core::image_content::hydrate_messages_for_execution(
        &restored,
        &mut rendered,
        MissingBlobBehavior::HistoricalPlaceholder,
    )
    .await?;
    assert_eq!(
        inline_images(&rendered),
        [base64::engine::general_purpose::STANDARD.encode(PNG)]
    );

    // Control: a state directory without the restored blobs renders the
    // same history with a placeholder.
    let empty = tempfile::tempdir()?;
    let unrestored_store = Base64BlobStoreAdapter::new(Arc::new(ObjectStoreBlobStore::local(
        empty.path().join("blobs"),
    )?));
    let mut unrestored = retained.messages().to_vec();
    meerkat_core::image_content::hydrate_messages_for_execution(
        &unrestored_store,
        &mut unrestored,
        MissingBlobBehavior::HistoricalPlaceholder,
    )
    .await?;
    assert!(inline_images(&unrestored).is_empty());
    assert!(serde_json::to_string(&unrestored)?.contains("[image unavailable"));
    Ok(())
}

/// A blob store with fixed rows; reading the id `failing` fails.
struct FakeBlobs {
    blobs: BTreeMap<String, ContinuityMirrorBlob>,
    failing: Option<String>,
}

#[async_trait::async_trait]
impl BlobRead for FakeBlobs {
    async fn read_blob(&self, blob_id: &str) -> Result<Option<ContinuityMirrorBlob>, String> {
        if self.failing.as_deref() == Some(blob_id) {
            return Err(format!("blob {blob_id}: the store failed"));
        }
        Ok(self.blobs.get(blob_id).cloned())
    }
}

fn fake_blobs(fills: &[char]) -> FakeBlobs {
    FakeBlobs {
        blobs: fills.iter().map(|fill| (id(*fill), row(*fill))).collect(),
        failing: None,
    }
}

/// How a [`RecordingSink`] answers a blob put.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BlobAnswer {
    Accept,
    Refuse,
    Hang,
}

/// A sink that records each blob it accepts, and answers the puts of one
/// blob id as told.
struct RecordingSink {
    accepted: Mutex<Vec<String>>,
    special: Option<(String, BlobAnswer)>,
}

impl RecordingSink {
    fn new(special: Option<(char, BlobAnswer)>) -> Self {
        Self {
            accepted: Mutex::new(Vec::new()),
            special: special.map(|(fill, answer)| (id(fill), answer)),
        }
    }

    fn take(&self) -> Vec<String> {
        self.accepted
            .lock()
            .map(|mut accepted| std::mem::take(&mut *accepted))
            .unwrap_or_default()
    }
}

#[async_trait::async_trait]
impl ContinuityMirrorSink for RecordingSink {
    async fn put(&self, _update: &ContinuityMirrorUpdate) -> Result<(), String> {
        Ok(())
    }

    async fn put_blob(&self, blob: &ContinuityMirrorBlob) -> Result<(), String> {
        let answer = match &self.special {
            Some((blob_id, answer)) if *blob_id == blob.blob_id => *answer,
            _ => BlobAnswer::Accept,
        };
        match answer {
            BlobAnswer::Accept => {
                let mut accepted = self.accepted.lock().map_err(|error| error.to_string())?;
                accepted.push(blob.blob_id.clone());
                Ok(())
            }
            BlobAnswer::Refuse => Err("the sink refused the blob".to_string()),
            BlobAnswer::Hang => std::future::pending().await,
        }
    }
}

fn ids(fills: &[char]) -> BTreeSet<String> {
    fills.iter().map(|fill| id(*fill)).collect()
}

const DEADLINE: Duration = Duration::from_secs(5);

#[tokio::test]
async fn new_blobs_are_put_once_each() -> TestResult {
    let blobs = fake_blobs(&['a', 'b', 'c']);
    let sink = RecordingSink::new(None);
    let counts = ContinuityMirrorCounts::default();
    let mut put_blobs = BTreeSet::new();

    let wanted = ids(&['a', 'b']);
    let put = put_new_blobs(&blobs, &sink, DEADLINE, &wanted, &mut put_blobs, &counts);
    assert!(put.await);
    assert_eq!(sink.take(), [id('a'), id('b')]);

    // A later session that references a and c puts only c.
    let wanted = ids(&['a', 'c']);
    let put = put_new_blobs(&blobs, &sink, DEADLINE, &wanted, &mut put_blobs, &counts);
    assert!(put.await);
    assert_eq!(sink.take(), [id('c')]);
    assert_eq!(counts.blobs.load(Ordering::Relaxed), 3);
    assert_eq!(put_blobs, ids(&['a', 'b', 'c']));
    Ok(())
}

#[tokio::test]
async fn a_blob_the_store_lacks_is_counted_and_never_holds_the_session() -> TestResult {
    let blobs = fake_blobs(&['a']);
    let sink = RecordingSink::new(None);
    let counts = ContinuityMirrorCounts::default();
    let mut put_blobs = BTreeSet::new();

    let wanted = ids(&['a', 'm']);
    let put = put_new_blobs(&blobs, &sink, DEADLINE, &wanted, &mut put_blobs, &counts);
    assert!(put.await, "the session goes out without the missing blob");
    assert_eq!(sink.take(), [id('a')]);
    assert_eq!(counts.missing_blobs.load(Ordering::Relaxed), 1);
    assert_eq!(counts.failures.load(Ordering::Relaxed), 0);
    assert_eq!(put_blobs, ids(&['a']));
    Ok(())
}

#[tokio::test]
async fn a_blob_the_sink_does_not_take_or_the_store_cannot_read_holds_the_session() -> TestResult {
    for (blobs, sink) in [
        (
            fake_blobs(&['a', 'b']),
            RecordingSink::new(Some(('a', BlobAnswer::Refuse))),
        ),
        (
            fake_blobs(&['a', 'b']),
            RecordingSink::new(Some(('a', BlobAnswer::Hang))),
        ),
        (
            FakeBlobs {
                failing: Some(id('a')),
                ..fake_blobs(&['a', 'b'])
            },
            RecordingSink::new(None),
        ),
    ] {
        let counts = ContinuityMirrorCounts::default();
        let mut put_blobs = BTreeSet::new();
        let wanted = ids(&['a', 'b']);
        let put = put_new_blobs(
            &blobs,
            &sink,
            Duration::from_millis(200),
            &wanted,
            &mut put_blobs,
            &counts,
        );
        assert!(!put.await, "the session waits for the next round");
        assert_eq!(counts.failures.load(Ordering::Relaxed), 1);
        assert!(put_blobs.is_empty());
        assert!(sink.take().is_empty());
    }
    Ok(())
}
