//! Note tools: `note_save`, `note_read`, `note_forget`, `note_list`.
//!
//! Failure convention (as for the file tools): store errors the model can act
//! on — bad name, too large, not found, user scope unavailable — become
//! `Ok(ToolResult { is_error: true, .. })`; malformed arguments (including an
//! unknown scope) become `Err(ToolError::InvalidArguments)`.

use super::store::{NoteError, NoteStore, Scope};
use async_trait::async_trait;
use meerkat_core::ops::ToolDispatchOutcome;
use meerkat_core::{
    AgentToolDispatcher, ToolCallView, ToolDef, ToolError, ToolMutationClass, ToolResult,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::fmt::Write as _;
use std::sync::Arc;

/// Tool name: save (create or replace) a note.
pub const NOTE_SAVE: &str = "note_save";
/// Tool name: read a note.
pub const NOTE_READ: &str = "note_read";
/// Tool name: delete a note.
pub const NOTE_FORGET: &str = "note_forget";
/// Tool name: list notes.
pub const NOTE_LIST: &str = "note_list";

/// Names of every tool exposed by [`NoteToolDispatcher`].
pub const NOTE_TOOL_NAMES: &[&str] = &[NOTE_SAVE, NOTE_READ, NOTE_FORGET, NOTE_LIST];

/// Provenance source id of the note tools (see [`crate::tools::FILE_TOOLS_SOURCE`]).
pub const NOTES_SOURCE: &str = "rkat-tui.notes";

const USER_UNAVAILABLE: &str = "user-level notes are unavailable (no config directory)";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct SaveArgs {
    #[schemars(
        description = "`user` for durable personal preferences that apply to every project; `workspace` for facts about this project."
    )]
    scope: Scope,
    #[schemars(
        description = "Short note name, e.g. `build-commands`. Lowercased; other characters become `-`. Saving an existing name replaces that note."
    )]
    name: String,
    #[schemars(
        description = "Note content (Markdown, at most 4 KiB). The first line is shown in the notes index, so make it a summary."
    )]
    content: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoteArgs {
    #[schemars(description = "Scope of the note: `user` or `workspace`.")]
    scope: Scope,
    #[schemars(description = "Note name, as shown by note_list.")]
    name: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    #[schemars(description = "Only list this scope (`user` or `workspace`); both when omitted.")]
    #[serde(default)]
    scope: Option<Scope>,
}

/// Why a note tool invocation failed in a way the model can recover from.
type Outcome = Result<String, String>;

struct Stores {
    user: Option<NoteStore>,
    workspace: NoteStore,
}

impl Stores {
    fn get(&self, scope: Scope) -> Result<&NoteStore, String> {
        match scope {
            Scope::User => self
                .user
                .as_ref()
                .ok_or_else(|| USER_UNAVAILABLE.to_string()),
            Scope::Workspace => Ok(&self.workspace),
        }
    }

    fn save(&self, args: SaveArgs) -> Outcome {
        let store = self.get(args.scope)?;
        let (slug, replaced) = store.save(&args.name, &args.content).map_err(describe)?;
        Ok(format!(
            "saved {}/{slug} ({})",
            args.scope,
            if replaced { "replaced" } else { "new" }
        ))
    }

    fn read(&self, args: NoteArgs) -> Outcome {
        self.get(args.scope)?.read(&args.name).map_err(describe)
    }

    fn forget(&self, args: NoteArgs) -> Outcome {
        let store = self.get(args.scope)?;
        store.forget(&args.name).map_err(describe)?;
        Ok(format!(
            "forgot {}/{}",
            args.scope,
            super::slug(&args.name).map_err(describe)?
        ))
    }

    fn list(&self, args: ListArgs) -> Outcome {
        let scopes: Vec<&NoteStore> = match args.scope {
            Some(scope) => vec![self.get(scope)?],
            None => self.user.iter().chain([&self.workspace]).collect(),
        };
        let mut out = String::new();
        for store in scopes {
            for note in store.list().map_err(describe)? {
                let _ = writeln!(out, "{}/{}: {}", store.scope(), note.name, note.first_line);
            }
        }
        if out.is_empty() {
            out.push_str("no notes saved");
        }
        if args.scope.is_none() && self.user.is_none() {
            let _ = write!(out, "\n({USER_UNAVAILABLE})");
        }
        Ok(out)
    }
}

fn describe(err: NoteError) -> String {
    err.to_string()
}

/// Dispatcher for the note tools.
pub struct NoteToolDispatcher {
    stores: Arc<Stores>,
    tools: Arc<[Arc<ToolDef>]>,
}

impl NoteToolDispatcher {
    /// Create a dispatcher over the user-level store (absent when there is no
    /// config directory) and the workspace store.
    #[must_use]
    pub fn new(user: Option<NoteStore>, workspace: NoteStore) -> Self {
        let tools: Arc<[Arc<ToolDef>]> = vec![
            Arc::new(ToolDef::new(
                NOTE_SAVE,
                "Save a note for future sessions (creates or replaces). Use \
                              scope `user` for durable personal preferences and `workspace` for \
                              facts about this project. Never store secrets or transient state; \
                              update an existing note rather than duplicating it."
                    .to_string(),
                meerkat_tools::schema_for::<SaveArgs>(),
            )),
            Arc::new(ToolDef::new(
                NOTE_READ,
                "Read the full content of a note.".to_string(),
                meerkat_tools::schema_for::<NoteArgs>(),
            )),
            Arc::new(ToolDef::new(
                NOTE_FORGET,
                "Delete a note that is wrong or no longer relevant.".to_string(),
                meerkat_tools::schema_for::<NoteArgs>(),
            )),
            Arc::new(ToolDef::new(
                NOTE_LIST,
                "List notes as `scope/name: first line`.".to_string(),
                meerkat_tools::schema_for::<ListArgs>(),
            )),
        ]
        .into_iter()
        .map(|def| {
            Arc::new(
                Arc::unwrap_or_clone(def).with_provenance(crate::tools::provenance(NOTES_SOURCE)),
            )
        })
        .collect();
        Self {
            stores: Arc::new(Stores { user, workspace }),
            tools,
        }
    }

    /// The user-level store, when available.
    #[must_use]
    pub fn user(&self) -> Option<&NoteStore> {
        self.stores.user.as_ref()
    }

    /// The workspace store.
    #[must_use]
    pub fn workspace(&self) -> &NoteStore {
        &self.stores.workspace
    }

    async fn run_blocking<A, F>(
        &self,
        call: ToolCallView<'_>,
        run: F,
    ) -> Result<ToolResult, ToolError>
    where
        A: DeserializeOwned + Send + 'static,
        F: FnOnce(&Stores, A) -> Outcome + Send + 'static,
    {
        let args: A = call
            .parse_args()
            .map_err(|err| ToolError::InvalidArguments {
                name: call.name.to_string(),
                reason: err.to_string(),
            })?;
        let stores = Arc::clone(&self.stores);
        let outcome = tokio::task::spawn_blocking(move || run(&stores, args))
            .await
            .map_err(|err| ToolError::ExecutionFailed {
                message: format!("{} worker failed: {err}", call.name),
            })?;
        let id = call.id.to_string();
        Ok(match outcome {
            Ok(text) => ToolResult::new(id, text, false),
            Err(message) => ToolResult::new(id, message, true),
        })
    }
}

impl std::fmt::Debug for NoteToolDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NoteToolDispatcher")
            .field("user", &self.stores.user)
            .field("workspace", &self.stores.workspace)
            .finish_non_exhaustive()
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for NoteToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    fn tool_mutation_class(&self, tool_name: &str) -> ToolMutationClass {
        match tool_name {
            NOTE_READ | NOTE_LIST => ToolMutationClass::ReadOnly,
            NOTE_SAVE | NOTE_FORGET => ToolMutationClass::Mutating,
            _ => ToolMutationClass::Unknown,
        }
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        let result = match call.name {
            NOTE_SAVE => self.run_blocking(call, Stores::save).await,
            NOTE_READ => self.run_blocking(call, Stores::read).await,
            NOTE_FORGET => self.run_blocking(call, Stores::forget).await,
            NOTE_LIST => self.run_blocking(call, Stores::list).await,
            other => Err(ToolError::NotFound {
                name: other.to_string(),
            }),
        };
        result.map(ToolDispatchOutcome::from)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn dispatcher(with_user: bool) -> (tempfile::TempDir, NoteToolDispatcher) {
        let tmp = tempfile::tempdir().unwrap();
        let user = with_user.then(|| NoteStore::open(tmp.path().join("user"), Scope::User));
        let ws = NoteStore::open(tmp.path().join("ws"), Scope::Workspace);
        (tmp, NoteToolDispatcher::new(user, ws))
    }

    async fn call(
        d: &NoteToolDispatcher,
        name: &str,
        args: Value,
    ) -> Result<ToolResult, ToolError> {
        let raw = serde_json::value::to_raw_value(&args).unwrap();
        d.dispatch(ToolCallView {
            id: "call-1",
            name,
            args: &raw,
        })
        .await
        .map(|outcome| outcome.result)
    }

    async fn text(d: &NoteToolDispatcher, name: &str, args: Value) -> (String, bool) {
        let r = call(d, name, args).await.unwrap();
        assert_eq!(r.tool_use_id, "call-1");
        (r.text_content(), r.is_error)
    }

    #[test]
    fn tool_names_match_definitions() {
        let (_tmp, d) = dispatcher(true);
        let names: Vec<_> = d.tools().iter().map(|t| t.name.clone()).collect();
        assert_eq!(names, NOTE_TOOL_NAMES);
    }

    #[tokio::test]
    async fn dispatch_each_tool() {
        let (_tmp, d) = dispatcher(true);
        let save = json!({"scope": "workspace", "name": "Build Cmd", "content": "make ci"});
        assert_eq!(
            text(&d, NOTE_SAVE, save.clone()).await,
            ("saved workspace/build-cmd (new)".into(), false)
        );
        assert_eq!(
            text(&d, NOTE_SAVE, save).await,
            ("saved workspace/build-cmd (replaced)".into(), false)
        );
        text(
            &d,
            NOTE_SAVE,
            json!({"scope": "user", "name": "style", "content": "terse\nmore"}),
        )
        .await;
        assert_eq!(
            text(
                &d,
                NOTE_READ,
                json!({"scope": "workspace", "name": "build-cmd"})
            )
            .await,
            ("make ci\n".into(), false)
        );
        assert_eq!(
            text(&d, NOTE_LIST, json!({})).await,
            (
                "user/style: terse\nworkspace/build-cmd: make ci\n".into(),
                false
            )
        );
        assert_eq!(
            text(&d, NOTE_LIST, json!({"scope": "user"})).await,
            ("user/style: terse\n".into(), false)
        );
        assert_eq!(
            text(&d, NOTE_FORGET, json!({"scope": "user", "name": "style"})).await,
            ("forgot user/style".into(), false)
        );
        let (msg, is_error) = text(&d, NOTE_READ, json!({"scope": "user", "name": "style"})).await;
        assert!(is_error);
        assert!(msg.contains("no note named"), "{msg}");
        let (msg, is_error) = text(
            &d,
            NOTE_SAVE,
            json!({"scope": "workspace", "name": "memory", "content": "x"}),
        )
        .await;
        assert!(is_error, "{msg}");
    }

    #[tokio::test]
    async fn malformed_arguments() {
        let (_tmp, d) = dispatcher(true);
        let err = call(&d, NOTE_READ, json!({"scope": "global", "name": "x"}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments { .. }), "{err:?}");
        let err = call(&d, NOTE_LIST, json!({"extra": 1})).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments { .. }), "{err:?}");
        let err = call(&d, "note_nope", json!({})).await.unwrap_err();
        assert!(matches!(err, ToolError::NotFound { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn user_scope_without_store() {
        let (_tmp, d) = dispatcher(false);
        let (msg, is_error) = text(
            &d,
            NOTE_SAVE,
            json!({"scope": "user", "name": "a", "content": "b"}),
        )
        .await;
        assert!(is_error);
        assert_eq!(msg, USER_UNAVAILABLE);
        let (msg, is_error) = text(&d, NOTE_LIST, json!({"scope": "user"})).await;
        assert!(is_error);
        assert_eq!(msg, USER_UNAVAILABLE);
        let (msg, is_error) = text(&d, NOTE_LIST, json!({})).await;
        assert!(!is_error);
        assert!(msg.starts_with("no notes saved"), "{msg}");
    }
}
