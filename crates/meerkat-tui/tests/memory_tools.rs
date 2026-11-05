//! End-to-end memory notes: approval tiers through [`classify_tier`], then
//! the actual write through `MemoryToolDispatcher`.
//!
//! The old crate decided through `ApprovalEngine` with a counting classifier;
//! the engine returns in P2, until then the tier alone shows whether the
//! classifier would be consulted (`Tier::Classifier`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use meerkat_core::{AgentToolDispatcher, ToolCallView};
use meerkat_tui::approval::policy::{Action, PolicyContext, Tier, classify_tier};
use meerkat_tui::approval::rules::RuleSet;
use meerkat_tui::memory::{
    INDEX_FILE, MEMORY_LIST, MEMORY_SAVE, MemoryToolDispatcher, NoteStore, Scope, prompt_index,
};
use meerkat_tui::paths::ProjectRoot;
use serde_json::{Value, json};

struct Harness {
    tmp: tempfile::TempDir,
    root: ProjectRoot,
}

fn harness() -> Harness {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let root = ProjectRoot::new(&project).unwrap();
    Harness { tmp, root }
}

impl Harness {
    fn tier(&self, name: &str, args: &Value) -> Tier {
        let ctx = PolicyContext {
            root: &self.root,
            trust_project_builds: true,
            posix_shell: true,
        };
        classify_tier(&Action::parse(name, args), &RuleSet::default(), &ctx).tier
    }

    fn stores(&self) -> (NoteStore, NoteStore) {
        (
            NoteStore::open(self.tmp.path().join("config/notes"), Scope::User),
            NoteStore::open(
                self.tmp.path().join("project/.rkat/tui/notes"),
                Scope::Workspace,
            ),
        )
    }
}

async fn dispatch(d: &MemoryToolDispatcher, name: &str, args: &Value) -> (String, bool) {
    let raw = serde_json::value::to_raw_value(args).unwrap();
    let result = d
        .dispatch(ToolCallView {
            id: "call-1",
            name,
            args: &raw,
        })
        .await
        .unwrap()
        .result;
    (result.text_content(), result.is_error)
}

#[tokio::test]
async fn workspace_save_is_allowed_without_classifier_and_lands_on_disk() {
    let h = harness();
    let args = json!({"scope": "workspace", "name": "Build", "content": "Run make ci.\nDetails."});
    assert_eq!(h.tier(MEMORY_SAVE, &args), Tier::InProjectEdit);
    assert_eq!(h.tier(MEMORY_LIST, &json!({})), Tier::AlwaysAllow);

    let (user, ws) = h.stores();
    let dispatcher = MemoryToolDispatcher::new(Some(user.clone()), ws.clone());
    let (text, is_error) = dispatch(&dispatcher, MEMORY_SAVE, &args).await;
    assert!(!is_error, "{text}");
    assert_eq!(text, "saved workspace/build (new)");
    assert_eq!(
        std::fs::read_to_string(ws.dir().join("build.md")).unwrap(),
        "Run make ci.\nDetails.\n"
    );
    let index = std::fs::read_to_string(ws.dir().join(INDEX_FILE)).unwrap();
    assert!(index.contains("- build: Run make ci."), "{index}");
    let prompt = prompt_index(Some(&user), &ws).unwrap();
    assert!(prompt.contains("## Workspace notes ("), "{prompt}");
    assert!(prompt.contains("- build: Run make ci."), "{prompt}");
    assert!(!prompt.contains("## User notes"), "{prompt}");
}

#[tokio::test]
async fn user_save_reaches_the_classifier_then_lands_on_disk() {
    let h = harness();
    let args = json!({"scope": "user", "name": "style", "content": "Prefer early returns."});
    assert_eq!(h.tier(MEMORY_SAVE, &args), Tier::Classifier);

    let (user, ws) = h.stores();
    let dispatcher = MemoryToolDispatcher::new(Some(user.clone()), ws.clone());
    let (text, is_error) = dispatch(&dispatcher, MEMORY_SAVE, &args).await;
    assert!(!is_error, "{text}");
    assert_eq!(text, "saved user/style (new)");
    assert!(user.dir().join("style.md").is_file());
    assert!(user.dir().join(INDEX_FILE).is_file());
    let prompt = prompt_index(Some(&user), &ws).unwrap();
    assert!(prompt.contains("## User notes ("), "{prompt}");
    assert!(
        prompt.contains("- style: Prefer early returns."),
        "{prompt}"
    );
}
