//! Storage-unification anti-ambient-resolution gate (plan Phase 5).
//!
//! One path authority: durable storage roots come from
//! `meerkat_core::StorageLayout` / the bootstrap resolver, not from ambient
//! process state. This gate bans ambient ROOT resolution in production code
//! outside the allowlisted bootstrap/layout/convention entries:
//!
//! - `dirs::*_dir()` getters — as qualified paths, `use`-imported bare calls
//!   (including renames and globs), and function-pointer references;
//! - `std::env::var` / `std::env::var_os` reads of `HOME`, `USERPROFILE`,
//!   `HOMEDRIVE`, `HOMEPATH`, `LOCALAPPDATA`, `APPDATA`, or any `XDG_*`
//!   variable — whether the callee is fully qualified, imported/renamed, or
//!   glob-imported, and whether the variable name is a string literal or a
//!   same-file `const`/`static`;
//! - `std::env::temp_dir()` (TMPDIR-derived ambient path resolution).
//!
//! Env-read detection requires the callee to actually resolve to `std::env`
//! (fully qualified, `use`-resolved, or an explicit `env::` path), so
//! unrelated `*::var` / `*::var_os` APIs do not false-positive on their
//! argument alone.
//!
//! Coverage follows the checked-in compilation surface: every `.rs` file
//! under the scanned crates' `src/` trees, each scanned crate's `build.rs`,
//! and checked-in `#[path = "..."]` / `include!("...")` targets reachable
//! from them (resolved lexically against the declaring file's directory).
//! Items gated `#[cfg(test)]` or `#[cfg(all(test, ...))]` are stripped
//! before detection; `#[cfg(any(test, ...))]` / `#[cfg(not(test))]` items
//! also compile outside tests and stay scanned.
//!
//! Deliberately NOT banned (scoped per the plan): feature-owned *relative*
//! paths (blob dirs, per-realm databases, projection files — banning every
//! path literal would create a storage god-module), `std::env::current_dir`
//! (legitimate execution-context uses: shell working dirs, tool project
//! dirs), and non-path environment reads (API keys, trace sinks).
//!
//! Honest residual limitations (accepted, not pretended away):
//! - cross-file constants and re-exports: a banned variable name or an
//!   ambient reader imported through another module/crate (`pub use`) is
//!   not propagated into call sites;
//! - macro-generated code: reads inside macro invocation token streams
//!   (e.g. `lazy_static!` bodies) are not expanded or visited;
//! - `env!`-composed `include!` targets (`concat!(env!("OUT_DIR"), ...)`)
//!   are build-generated and do not exist at gate time — out of scope;
//! - env-reader functions passed as values (`iter.map(std::env::var)`)
//!   where the variable name flows in as data, not as a literal/const;
//! - `#[path]`/`include!` resolution is lexical relative to the declaring
//!   file (inline-module nesting nuances are not modeled) and only the
//!   rustfmt-normalized `#[path = "..."]` spelling is discovered;
//! - files named `tests.rs` / `*_tests.rs` are `#[cfg(test)]`-included
//!   module files by repo convention and are skipped like `tests/` trees.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use syn::spanned::Spanned;
use syn::visit::Visit;

use crate::effect_authority::push_finding;

/// How much of a file an allowlist entry exempts.
#[derive(Clone, Copy)]
enum AllowScope {
    /// The entire file. Reserved for the layout/bootstrap path authority
    /// itself (enforced by a test below).
    WholeFile,
    /// Named functions only: `name` for free functions, `Type::name` for
    /// impl/trait methods. Names match per file (module nesting inside the
    /// file is not encoded). Anything nested in an allowed function
    /// (closures, local items) is covered; module-level reads are not.
    Functions(&'static [&'static str]),
}

/// `(repo-relative file, exempted scope, sanctioned reason)`.
type AllowEntry = (&'static str, AllowScope, &'static str);

/// Files/functions allowed to resolve ambient roots, with the sanctioned
/// reason. Everything here is either the layout/bootstrap authority itself
/// or a documented storage convention the plan preserves in place.
const AMBIENT_ALLOWLIST: [AllowEntry; 10] = [
    // The path authority and the bootstrap resolver: where ambient inputs
    // are turned into the immutable layout. The only whole-file entries.
    (
        "meerkat-core/src/storage_layout.rs",
        AllowScope::WholeFile,
        "the path authority",
    ),
    (
        "meerkat-core/src/runtime_bootstrap.rs",
        AllowScope::WholeFile,
        "default_state_root / dual-root candidates",
    ),
    // Deprecated ambient wrapper kept through its compatibility window
    // (no in-repo production callers; removal follows the deprecation
    // cadence).
    (
        "meerkat-skills/src/resolve.rs",
        AllowScope::Functions(&["resolve_repositories"]),
        "deprecated ambient wrapper (resolve_repositories)",
    ),
    // User-global config/mcp document conventions (home-rooted `.rkat`),
    // consumed via the layout's user_home_root at surface level; the
    // ambient forms remain for SDK compatibility.
    (
        "meerkat-core/src/config.rs",
        AllowScope::Functions(&[
            "Config::load",
            "Config::load_layered_hooks",
            "Config::global_config_path",
        ]),
        "global config doc convention (~/.rkat/config.toml)",
    ),
    (
        "meerkat-core/src/mcp_config.rs",
        AllowScope::Functions(&["user_mcp_path", "user_mcp_dir"]),
        "user mcp.toml convention (~/.rkat/mcp.toml)",
    ),
    // Durable key material with an exactly-preserved hand-rolled platform
    // resolution (porting it to `dirs` would silently relocate — and thereby
    // rotate — comms identity keys).
    (
        "meerkat/src/sdk.rs",
        AllowScope::Functions(&["canonical_session_comms_identity_root"]),
        "session-comms identity root (resolution preserved exactly)",
    ),
    // Credentials convention: config_dir/meerkat/credentials, contractually
    // unchanged by the storage unification.
    (
        "meerkat-auth-core/src/auth_store/mod.rs",
        AllowScope::Functions(&[
            "TokenStoreBackend::default_keyring_auto",
            "TokenStoreBackend::default_file",
            "TokenStoreBackend::refresh_lock_dir",
        ]),
        "credentials root convention",
    ),
    // Foreign credential convention (Google ADC reads gcloud's own path).
    (
        "meerkat-auth-core/src/authorizers/google.rs",
        AllowScope::Functions(&["GoogleAuthAuthorizer::with_env_lookup"]),
        "third-party ADC convention",
    ),
    // Surface bootstrap entrypoints: ambient inputs (home default for
    // --user-config-root, ~ expansion of user-typed paths) are gathered
    // in these functions and threaded explicitly from then on.
    (
        "meerkat-cli/src/main.rs",
        AllowScope::Functions(&[
            "resolve_user_prompt_file_token",
            "expand_path",
            "resolve_runtime_scope_with_realm",
            "cli_global_config_path",
            "interactive_login",
            "handle_storage_migrate",
            "run_rpc_surface",
        ]),
        "CLI bootstrap inputs",
    ),
    (
        "meerkat-rpc/src/main.rs",
        AllowScope::Functions(&["async_main"]),
        "RPC bootstrap inputs",
    ),
];

/// Env vars whose reads constitute ambient root resolution. `XDG_*` is
/// banned as a whole family (see `is_banned_env_var`).
const BANNED_ENV_VARS: [&str; 7] = [
    "HOME",
    "XDG_STATE_HOME",
    "LOCALAPPDATA",
    "APPDATA",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
];

fn is_banned_env_var(name: &str) -> bool {
    BANNED_ENV_VARS.contains(&name) || name.starts_with("XDG_")
}

const GATE_LABEL: &str = "storage-ambient gate: ambient root resolution (dirs::* / std::env::temp_dir / HOME-family / XDG_* / LOCALAPPDATA / APPDATA reads) belongs in the bootstrap/layout modules — thread roots from meerkat_core::StorageLayout instead";

/// Crate source roots scanned by the gate (production library/binary code;
/// the walker skips tests/, examples/, benches/, generated code). Each
/// crate's `build.rs` is scanned alongside its `src/` tree.
const SCAN_ROOTS: [&str; 20] = [
    "meerkat-core/src",
    "meerkat-store/src",
    "meerkat-sqlite/src",
    "meerkat-session/src",
    "meerkat-memory/src",
    "meerkat-tools/src",
    "meerkat-schedule/src",
    "meerkat-workgraph/src",
    "meerkat-mob/src",
    "meerkat-mob-mcp/src",
    "meerkat-mob-pack/src",
    "meerkat-runtime/src",
    "meerkat-skills/src",
    "meerkat-hooks/src",
    "meerkat-comms/src",
    "meerkat/src",
    "meerkat-cli/src",
    "meerkat-rpc/src",
    "meerkat-rest/src",
    "meerkat-mcp-server/src",
];

pub fn run_storage_ambient_gate() -> Result<()> {
    let root = crate::public_contracts::repo_root()?;
    let findings = collect_storage_ambient_findings(&root)?;
    if findings.is_empty() {
        println!("storage-ambient gate clean");
        return Ok(());
    }
    for finding in &findings {
        eprintln!("{finding}");
    }
    bail!("storage-ambient gate failed: {} finding(s)", findings.len());
}

pub fn collect_storage_ambient_findings(root: &Path) -> Result<Vec<String>> {
    collect_findings_for_roots(root, &SCAN_ROOTS, &AMBIENT_ALLOWLIST)
}

fn collect_findings_for_roots(
    root: &Path,
    scan_roots: &[&str],
    allowlist: &[AllowEntry],
) -> Result<Vec<String>> {
    let mut pending: Vec<PathBuf> = Vec::new();
    for scan_root in scan_roots {
        let dir = root.join(scan_root);
        if dir.exists() {
            walk_rs_files(&dir, &mut |path| {
                pending.push(path.to_path_buf());
                Ok(())
            })?;
        }
        // Build scripts are production code compiled and run by cargo; they
        // live beside `src`, outside the directory walk.
        if let Some(crate_dir) = scan_root.strip_suffix("/src") {
            let build_script = root.join(crate_dir).join("build.rs");
            if build_script.exists() {
                pending.push(build_script);
            }
        }
    }
    let mut findings = Vec::new();
    let mut visited: HashSet<PathBuf> = HashSet::new();
    // Worklist: scanning a file can enqueue its checked-in `#[path]` /
    // `include!` targets, which may live outside the walked directories.
    while let Some(path) = pending.pop() {
        let path = normalize_path(&path);
        if !visited.insert(path.clone()) {
            continue;
        }
        scan_file(root, &path, allowlist, &mut findings, &mut pending)?;
    }
    findings.sort();
    findings.dedup();
    Ok(findings)
}

fn scan_file(
    root: &Path,
    path: &Path,
    allowlist: &[AllowEntry],
    findings: &mut Vec<String>,
    pending: &mut Vec<PathBuf>,
) -> Result<()> {
    let rel = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let mut allowed_fns: &[&str] = &[];
    if let Some((_, scope, _)) = allowlist.iter().find(|(file, _, _)| rel == *file) {
        match *scope {
            AllowScope::WholeFile => return Ok(()),
            AllowScope::Functions(functions) => allowed_fns = functions,
        }
    }
    let source = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    // Cheap pre-filter before paying for a parse; `prefilter_hits` is kept a
    // strict superset of everything the visitor can flag.
    if !prefilter_hits(&source) {
        return Ok(());
    }
    let base_dir = path.parent().unwrap_or(Path::new("."));
    let targets = analyze_source(&rel, &source, base_dir, allowed_fns, findings)?;
    for target in targets {
        // Only checked-in files are followable; generated OUT_DIR sources do
        // not exist at gate time (documented out of scope).
        if target.exists() {
            pending.push(target);
        }
    }
    Ok(())
}

/// Raw-text needles that MUST appear in any file the visitor could flag:
/// dirs-crate resolution requires the `dirs` token (qualified path or `use`
/// import), `std::env::temp_dir` resolution requires the `temp_dir` token,
/// and every flaggable env read carries the banned variable name as a
/// string literal in the same file (at the call site or in a local const) —
/// so the variable-name needles cover aliased and const-fed reads too.
/// `include!` / `#[path` keep re-inclusion discovery alive. A prefilter
/// miss therefore means the visitor cannot fire, and skipping the parse is
/// sound.
fn prefilter_hits(source: &str) -> bool {
    const NEEDLES: [&str; 5] = ["dirs", "temp_dir", "XDG_", "include!", "#[path"];
    NEEDLES.iter().any(|needle| source.contains(needle))
        || BANNED_ENV_VARS.iter().any(|var| source.contains(var))
}

/// Parses `source`, strips test-only items, and runs both passes (symbol
/// tables, then detection). Returns the file's checked-in re-inclusion
/// targets, resolved lexically against `base_dir`.
fn analyze_source(
    rel: &str,
    source: &str,
    base_dir: &Path,
    allowed_fns: &[&str],
    findings: &mut Vec<String>,
) -> Result<Vec<PathBuf>> {
    // Fail closed on unparseable production code.
    let mut parsed = syn::parse_file(source)
        .with_context(|| format!("storage-ambient gate cannot parse {rel}"))?;
    strip_test_only_items(&mut parsed.items);
    let mut tables = SymbolTables::default();
    tables.visit_file(&parsed);
    let mut targets = Vec::new();
    let mut collector = ReinclusionCollector {
        base_dir,
        targets: &mut targets,
    };
    collector.visit_file(&parsed);
    let mut visitor = AmbientVisitor {
        rel,
        findings,
        tables: &tables,
        allowed_fns,
        fn_stack: Vec::new(),
        type_stack: Vec::new(),
    };
    visitor.visit_file(&parsed);
    Ok(targets)
}

/// Removes items that only compile under `cfg(test)` — plain `#[cfg(test)]`
/// and `#[cfg(all(test, ...))]` forms — before detection; test-only code is
/// not production surface. `any(test, ...)` / `not(test)` items also compile
/// outside tests and are kept.
fn strip_test_only_items(items: &mut Vec<syn::Item>) {
    items.retain(|item| !test_only_item(item));
    for item in items.iter_mut() {
        match item {
            syn::Item::Mod(module) => {
                if let Some((_, inner)) = module.content.as_mut() {
                    strip_test_only_items(inner);
                }
            }
            syn::Item::Impl(item_impl) => {
                item_impl.items.retain(|impl_item| {
                    let attrs = match impl_item {
                        syn::ImplItem::Const(item) => &item.attrs,
                        syn::ImplItem::Fn(item) => &item.attrs,
                        syn::ImplItem::Type(item) => &item.attrs,
                        syn::ImplItem::Macro(item) => &item.attrs,
                        _ => return true,
                    };
                    !attrs.iter().any(attr_requires_test)
                });
            }
            _ => {}
        }
    }
}

fn test_only_item(item: &syn::Item) -> bool {
    let attrs = match item {
        syn::Item::Const(item) => &item.attrs,
        syn::Item::Enum(item) => &item.attrs,
        syn::Item::ExternCrate(item) => &item.attrs,
        syn::Item::Fn(item) => &item.attrs,
        syn::Item::ForeignMod(item) => &item.attrs,
        syn::Item::Impl(item) => &item.attrs,
        syn::Item::Macro(item) => &item.attrs,
        syn::Item::Mod(item) => &item.attrs,
        syn::Item::Static(item) => &item.attrs,
        syn::Item::Struct(item) => &item.attrs,
        syn::Item::Trait(item) => &item.attrs,
        syn::Item::TraitAlias(item) => &item.attrs,
        syn::Item::Type(item) => &item.attrs,
        syn::Item::Union(item) => &item.attrs,
        syn::Item::Use(item) => &item.attrs,
        _ => return false,
    };
    attrs.iter().any(attr_requires_test)
}

/// True when the attribute is a `#[cfg(...)]` whose predicate REQUIRES the
/// `test` cfg: `test` itself, or an `all(...)` with a requiring element.
fn attr_requires_test(attr: &syn::Attribute) -> bool {
    if !attr.path().is_ident("cfg") {
        return false;
    }
    attr.parse_args::<syn::Meta>()
        .is_ok_and(|predicate| predicate_requires_test(&predicate))
}

fn predicate_requires_test(predicate: &syn::Meta) -> bool {
    match predicate {
        syn::Meta::Path(path) => path.is_ident("test"),
        syn::Meta::List(list) if list.path.is_ident("all") => list
            .parse_args_with(
                syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
            )
            .is_ok_and(|nested| nested.iter().any(predicate_requires_test)),
        _ => false,
    }
}

fn walk_rs_files(dir: &Path, visit: &mut dyn FnMut(&Path) -> Result<()>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read dir {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if matches!(
                name.as_ref(),
                "tests" | "examples" | "benches" | "generated" | "target"
            ) {
                continue;
            }
            walk_rs_files(&path, visit)?;
        } else if name.ends_with(".rs") {
            // `tests.rs` / `*_tests.rs` are `#[cfg(test)]`-included module
            // files by repo convention; the cfg strip cannot see the
            // attribute (it lives on the `mod` declaration in the parent),
            // so skip them like `tests/` trees.
            if name.as_ref() == "tests.rs" || name.ends_with("_tests.rs") {
                continue;
            }
            visit(&path)?;
        }
    }
    Ok(())
}

/// Lexical normalization (no filesystem access): resolves `.` and `..`
/// components so re-included targets and walked paths dedupe to one key and
/// produce stable repo-relative finding paths.
fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

/// Per-file symbol tables: `use`-tree imports (including renames), glob
/// import prefixes, and string `const`/`static` bindings — the inputs to
/// name resolution and local const propagation.
#[derive(Default)]
struct SymbolTables {
    /// Local name -> full imported path segments (e.g. `home_dir` ->
    /// `["dirs", "home_dir"]`, `env` -> `["std", "env"]`).
    imports: HashMap<String, Vec<String>>,
    /// Prefixes of glob imports (e.g. `use std::env::*` -> `["std", "env"]`).
    globs: Vec<Vec<String>>,
    /// String constants: name -> literal value.
    consts: HashMap<String, String>,
}

impl SymbolTables {
    fn flatten_use_tree(&mut self, tree: &syn::UseTree, prefix: &mut Vec<String>) {
        match tree {
            syn::UseTree::Path(path) => {
                prefix.push(path.ident.to_string());
                self.flatten_use_tree(&path.tree, prefix);
                prefix.pop();
            }
            syn::UseTree::Name(name) => {
                let ident = name.ident.to_string();
                if ident == "self" {
                    // `use std::env::{self}` binds the last prefix segment.
                    if let Some(last) = prefix.last().cloned() {
                        self.imports.insert(last, prefix.clone());
                    }
                } else {
                    let mut full = prefix.clone();
                    full.push(ident.clone());
                    self.imports.insert(ident, full);
                }
            }
            syn::UseTree::Rename(rename) => {
                let mut full = prefix.clone();
                if rename.ident != "self" {
                    full.push(rename.ident.to_string());
                }
                self.imports.insert(rename.rename.to_string(), full);
            }
            syn::UseTree::Glob(_) => self.globs.push(prefix.clone()),
            syn::UseTree::Group(group) => {
                for tree in &group.items {
                    self.flatten_use_tree(tree, prefix);
                }
            }
        }
    }

    fn record_str_binding(&mut self, ident: &syn::Ident, expr: &syn::Expr) {
        if let syn::Expr::Lit(lit) = expr
            && let syn::Lit::Str(value) = &lit.lit
        {
            self.consts.insert(ident.to_string(), value.value());
        }
    }
}

impl<'ast> Visit<'ast> for SymbolTables {
    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        let mut prefix = Vec::new();
        self.flatten_use_tree(&node.tree, &mut prefix);
    }

    fn visit_item_const(&mut self, node: &'ast syn::ItemConst) {
        self.record_str_binding(&node.ident, &node.expr);
        syn::visit::visit_item_const(self, node);
    }

    fn visit_item_static(&mut self, node: &'ast syn::ItemStatic) {
        self.record_str_binding(&node.ident, &node.expr);
        syn::visit::visit_item_static(self, node);
    }

    fn visit_impl_item_const(&mut self, node: &'ast syn::ImplItemConst) {
        self.record_str_binding(&node.ident, &node.expr);
        syn::visit::visit_impl_item_const(self, node);
    }

    fn visit_trait_item_const(&mut self, node: &'ast syn::TraitItemConst) {
        if let Some((_, expr)) = &node.default {
            self.record_str_binding(&node.ident, expr);
        }
        syn::visit::visit_trait_item_const(self, node);
    }
}

/// Collects checked-in re-inclusion targets: out-of-line `#[path = "..."]`
/// module declarations and `include!("literal.rs")` invocations. Runs after
/// the cfg-test strip, so `#[cfg(test)]` inclusions are not followed.
struct ReinclusionCollector<'a> {
    base_dir: &'a Path,
    targets: &'a mut Vec<PathBuf>,
}

impl<'ast> Visit<'ast> for ReinclusionCollector<'_> {
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if node.content.is_none()
            && let Some(target) = mod_path_attr(&node.attrs)
        {
            self.targets
                .push(normalize_path(&self.base_dir.join(target)));
        }
        syn::visit::visit_item_mod(self, node);
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        if node
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "include")
            && let Ok(literal) = syn::parse2::<syn::LitStr>(node.tokens.clone())
        {
            let target = literal.value();
            // `include!` targets are Rust source; a non-`.rs` target (or a
            // `concat!`/`env!` composition, which fails the literal parse
            // above) is skipped rather than parsed speculatively.
            if Path::new(&target)
                .extension()
                .is_some_and(|ext| ext == "rs")
            {
                self.targets
                    .push(normalize_path(&self.base_dir.join(target)));
            }
        }
        syn::visit::visit_macro(self, node);
    }
}

fn mod_path_attr(attrs: &[syn::Attribute]) -> Option<String> {
    attrs.iter().find_map(|attr| {
        if !attr.path().is_ident("path") {
            return None;
        }
        match &attr.meta {
            syn::Meta::NameValue(name_value) => match &name_value.value {
                syn::Expr::Lit(lit) => match &lit.lit {
                    syn::Lit::Str(value) => Some(value.value()),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        }
    })
}

/// Substitutes the path's first segment through the file's import map (so
/// `home_dir` resolves to `dirs::home_dir`, `env::var` to `std::env::var`).
/// Leading-colon (`::x`) paths bypass imports by language rule.
fn resolve_segments(path: &syn::Path, imports: &HashMap<String, Vec<String>>) -> Vec<String> {
    let segments: Vec<String> = path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect();
    if path.leading_colon.is_none()
        && let Some(first) = segments.first()
        && let Some(target) = imports.get(first)
    {
        let mut resolved = target.clone();
        resolved.extend(segments.into_iter().skip(1));
        return resolved;
    }
    segments
}

/// `dirs::home_dir` / `dirs::config_dir` / ... — the `dirs` crate must be
/// the path root (a `crate::dirs`-style local module does not qualify).
fn is_dirs_getter(resolved: &[String]) -> bool {
    resolved.len() >= 2
        && resolved[0] == "dirs"
        && resolved
            .last()
            .is_some_and(|segment| segment.ends_with("_dir"))
}

/// Resolved path names one of `names` under `std::env` (or an explicit
/// `env::` path, which cannot exist without importing `std::env` or
/// shadowing it locally — flagged conservatively).
fn is_std_env_fn(resolved: &[String], names: &[&str]) -> bool {
    match resolved {
        [root, name] => root == "env" && names.contains(&name.as_str()),
        [root, module, name] => root == "std" && module == "env" && names.contains(&name.as_str()),
        _ => false,
    }
}

fn glob_of(globs: &[Vec<String>], target: &[&str]) -> bool {
    globs
        .iter()
        .any(|prefix| prefix.iter().map(String::as_str).eq(target.iter().copied()))
}

struct AmbientVisitor<'a> {
    rel: &'a str,
    findings: &'a mut Vec<String>,
    tables: &'a SymbolTables,
    allowed_fns: &'a [&'a str],
    fn_stack: Vec<String>,
    type_stack: Vec<String>,
}

impl AmbientVisitor<'_> {
    fn record(&mut self, span: proc_macro2::Span) {
        // Function-scoped allowances cover the named function and anything
        // nested inside it; module-level reads only fall to whole-file
        // entries (checked before parsing).
        if self
            .fn_stack
            .iter()
            .any(|name| self.allowed_fns.contains(&name.as_str()))
        {
            return;
        }
        push_finding(self.findings, self.rel, span, GATE_LABEL);
    }

    fn push_method(&mut self, ident: &syn::Ident) {
        let name = match self.type_stack.last() {
            Some(ty) => format!("{ty}::{ident}"),
            None => ident.to_string(),
        };
        self.fn_stack.push(name);
    }

    /// Bare idents brought in by glob imports (`use dirs::*` /
    /// `use std::env::*`) that resolve to an ambient path getter.
    fn glob_resolves_ambient(&self, ident: &str) -> bool {
        (ident.ends_with("_dir") && glob_of(&self.tables.globs, &["dirs"]))
            || (ident == "temp_dir" && glob_of(&self.tables.globs, &["std", "env"]))
    }

    /// The env-var name an argument expression denotes, if statically
    /// known: a string literal, or a same-file `const`/`static` (also via
    /// `Self::NAME`). Cross-file constants are a documented residual gap.
    fn env_name_of(&self, expr: &syn::Expr) -> Option<String> {
        match expr {
            syn::Expr::Lit(lit) => match &lit.lit {
                syn::Lit::Str(name) => Some(name.value()),
                _ => None,
            },
            syn::Expr::Reference(reference) => self.env_name_of(&reference.expr),
            syn::Expr::Paren(paren) => self.env_name_of(&paren.expr),
            syn::Expr::Group(group) => self.env_name_of(&group.expr),
            syn::Expr::Path(path) => {
                let segments = &path.path.segments;
                let ident = match segments.len() {
                    1 => segments.first()?.ident.to_string(),
                    2 if segments.first()?.ident == "Self" => segments.last()?.ident.to_string(),
                    _ => return None,
                };
                self.tables.consts.get(&ident).cloned()
            }
            _ => None,
        }
    }
}

impl<'ast> Visit<'ast> for AmbientVisitor<'_> {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        self.fn_stack.push(node.sig.ident.to_string());
        syn::visit::visit_item_fn(self, node);
        self.fn_stack.pop();
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        self.push_method(&node.sig.ident);
        syn::visit::visit_impl_item_fn(self, node);
        self.fn_stack.pop();
    }

    fn visit_trait_item_fn(&mut self, node: &'ast syn::TraitItemFn) {
        self.push_method(&node.sig.ident);
        syn::visit::visit_trait_item_fn(self, node);
        self.fn_stack.pop();
    }

    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        self.type_stack.push(self_type_name(&node.self_ty));
        syn::visit::visit_item_impl(self, node);
        self.type_stack.pop();
    }

    fn visit_item_trait(&mut self, node: &'ast syn::ItemTrait) {
        self.type_stack.push(node.ident.to_string());
        syn::visit::visit_item_trait(self, node);
        self.type_stack.pop();
    }

    fn visit_path(&mut self, node: &'ast syn::Path) {
        // Path-level (not call-level) detection so function-pointer uses
        // (`.or_else(dirs::home_dir)`) are caught alongside direct calls.
        let resolved = resolve_segments(node, &self.tables.imports);
        let ambient = is_dirs_getter(&resolved)
            || is_std_env_fn(&resolved, &["temp_dir"])
            || (resolved.len() == 1 && self.glob_resolves_ambient(&resolved[0]));
        if ambient {
            self.record(node.span());
        }
        syn::visit::visit_path(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        // `env::var("HOME")` / aliased / glob-imported forms — an env read
        // whose callee resolves to std::env and whose variable name is a
        // banned literal or same-file constant.
        if let syn::Expr::Path(callee) = node.func.as_ref() {
            let resolved = resolve_segments(&callee.path, &self.tables.imports);
            let env_read = is_std_env_fn(&resolved, &["var", "var_os"])
                || (resolved.len() == 1
                    && matches!(resolved[0].as_str(), "var" | "var_os")
                    && glob_of(&self.tables.globs, &["std", "env"]));
            if env_read
                && let Some(arg) = node.args.first()
                && let Some(name) = self.env_name_of(arg)
                && is_banned_env_var(&name)
            {
                self.record(node.span());
            }
        }
        syn::visit::visit_expr_call(self, node);
    }
}

fn self_type_name(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(type_path) => type_path
            .path
            .segments
            .last()
            .map_or_else(|| "_".to_string(), |segment| segment.ident.to_string()),
        _ => "_".to_string(),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn fixture_findings_with_allow(source: &str, allowed_fns: &[&str]) -> Vec<String> {
        let mut findings = Vec::new();
        let _targets = analyze_source(
            "fixture.rs",
            source,
            Path::new("."),
            allowed_fns,
            &mut findings,
        )
        .expect("fixture parses");
        // Superset contract: anything the visitor flags must survive the
        // raw-text prefilter, otherwise the file would be skipped unparsed.
        if !findings.is_empty() {
            assert!(
                prefilter_hits(source),
                "prefilter skipped a flaggable fixture:\n{source}"
            );
        }
        findings
    }

    fn fixture_findings(source: &str) -> Vec<String> {
        fixture_findings_with_allow(source, &[])
    }

    #[test]
    fn gate_is_clean_on_the_current_tree() {
        let root = crate::public_contracts::repo_root().expect("repo root");
        let findings = collect_storage_ambient_findings(&root).expect("scan");
        assert!(
            findings.is_empty(),
            "storage-ambient gate found ambient root resolution:\n{}",
            findings.join("\n")
        );
    }

    #[test]
    fn whole_file_exemptions_are_reserved_for_the_layout_authority() {
        const LAYOUT_AUTHORITY: [&str; 2] = [
            "meerkat-core/src/storage_layout.rs",
            "meerkat-core/src/runtime_bootstrap.rs",
        ];
        for (file, scope, _) in &AMBIENT_ALLOWLIST {
            if matches!(scope, AllowScope::WholeFile) {
                assert!(
                    LAYOUT_AUTHORITY.contains(file),
                    "whole-file ambient exemption outside the layout authority: {file}"
                );
            }
        }
    }

    #[test]
    fn visitor_flags_dirs_calls_and_home_reads() {
        let source = r#"
            pub fn bad_root() -> Option<std::path::PathBuf> {
                dirs::home_dir()
            }
            pub fn bad_env() -> Option<std::ffi::OsString> {
                std::env::var_os("HOME")
            }
            pub fn fine_env() -> Result<String, std::env::VarError> {
                std::env::var("ANTHROPIC_API_KEY")
            }
            #[cfg(test)]
            pub fn test_only() -> Option<std::path::PathBuf> {
                dirs::home_dir()
            }
        "#;
        assert_eq!(fixture_findings(source).len(), 2);
    }

    #[test]
    fn cfg_all_test_items_are_stripped_but_not_any_or_not() {
        let source = r#"
            #[cfg(all(test, feature = "comms"))]
            mod gated_tests {
                pub fn t() -> Option<std::path::PathBuf> {
                    dirs::home_dir()
                }
            }
            #[cfg(not(test))]
            pub fn production() -> Option<std::path::PathBuf> {
                dirs::home_dir()
            }
            #[cfg(any(test, feature = "test-support"))]
            pub fn support() -> Option<std::path::PathBuf> {
                dirs::home_dir()
            }
        "#;
        assert_eq!(fixture_findings(source).len(), 2);
    }

    #[test]
    fn every_xdg_variable_is_caught_and_prefiltered() {
        let source = r#"
            pub fn xdg_family() {
                let _ = std::env::var_os("XDG_RUNTIME_DIR");
                let _ = std::env::var("XDG_CONFIG_DIRS");
            }
        "#;
        assert!(prefilter_hits(source), "prefilter must catch XDG_* files");
        assert_eq!(fixture_findings(source).len(), 2);
    }

    #[test]
    fn windows_ambient_home_variants_are_banned() {
        let source = r#"
            pub fn windows_roots() {
                let _ = std::env::var_os("USERPROFILE");
                let _ = std::env::var("HOMEDRIVE");
                let _ = std::env::var("HOMEPATH");
                let _ = std::env::var_os("APPDATA");
            }
        "#;
        assert_eq!(fixture_findings(source).len(), 4);
    }

    #[test]
    fn imported_and_aliased_readers_are_flagged() {
        let source = r#"
            use dirs::home_dir;
            use dirs::config_dir as cfg_dir;
            use std::env::var_os as read_env;
            use std::env;
            pub fn bypasses() {
                let _ = home_dir();
                let _ = cfg_dir();
                let _ = read_env("HOME");
                let _ = env::var("HOME");
            }
        "#;
        let findings = fixture_findings(source);
        assert_eq!(findings.len(), 4, "{findings:?}");
    }

    #[test]
    fn function_pointer_uses_are_flagged() {
        let source = r"
            use dirs::config_dir;
            pub fn pointers(fallback: Option<std::path::PathBuf>) -> Option<std::path::PathBuf> {
                fallback.or_else(dirs::home_dir).or_else(config_dir)
            }
        ";
        assert_eq!(fixture_findings(source).len(), 2);
    }

    #[test]
    fn local_const_names_flow_into_env_reads() {
        let source = r#"
            const HOME_KEY: &str = "HOME";
            static STATE_KEY: &str = "XDG_STATE_HOME";
            pub fn consts() {
                let _ = std::env::var_os(HOME_KEY);
                let _ = std::env::var(STATE_KEY);
                let _ = std::env::var(&HOME_KEY);
            }
        "#;
        assert_eq!(fixture_findings(source).len(), 3);
    }

    #[test]
    fn glob_imports_are_flagged() {
        let env_glob = r#"
            use std::env::*;
            pub fn globbed() {
                let _ = var("HOME");
            }
        "#;
        assert_eq!(fixture_findings(env_glob).len(), 1);
        let dirs_glob = r"
            use dirs::*;
            pub fn globbed() -> Option<std::path::PathBuf> {
                home_dir()
            }
        ";
        assert_eq!(fixture_findings(dirs_glob).len(), 1);
    }

    #[test]
    fn std_env_temp_dir_is_ambient_resolution() {
        let source = r"
            use std::env::temp_dir;
            pub fn temp_roots() {
                let _ = std::env::temp_dir();
                let _ = temp_dir();
            }
        ";
        assert_eq!(fixture_findings(source).len(), 2);
        let benign = r#"
            pub fn benign() {
                let temp_dir = std::path::PathBuf::from("scratch");
                let _ = temp_dir.join("x");
            }
        "#;
        assert!(fixture_findings(benign).is_empty());
    }

    #[test]
    fn unrelated_var_functions_are_not_flagged() {
        let source = r#"
            mod settings {
                pub fn var(_key: &str) -> Option<String> {
                    None
                }
            }
            use crate::paths as dirs;
            fn var(_key: &str) -> Option<String> {
                None
            }
            pub fn lookalikes() {
                let _ = settings::var("HOME");
                let _ = var("HOME");
                let _ = crate::ambient::dirs::data_dir();
                let _ = dirs::data_dir();
            }
        "#;
        let findings = fixture_findings_with_allow(source, &[]);
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn function_scoped_allowlist_only_covers_named_functions() {
        let source = r"
            pub fn sanctioned() -> Option<std::path::PathBuf> {
                dirs::home_dir()
            }
            pub fn rogue() -> Option<std::path::PathBuf> {
                dirs::home_dir()
            }
            pub struct Cfg;
            impl Cfg {
                pub fn load() -> Option<std::path::PathBuf> {
                    dirs::home_dir()
                }
                pub fn other() -> Option<std::path::PathBuf> {
                    dirs::home_dir()
                }
            }
        ";
        let findings = fixture_findings_with_allow(source, &["sanctioned", "Cfg::load"]);
        assert_eq!(findings.len(), 2, "{findings:?}");
    }

    #[test]
    fn module_level_reads_are_not_function_allowlistable() {
        let source = r"
            pub static HOME_RESOLVER: fn() -> Option<std::path::PathBuf> = dirs::home_dir;
        ";
        let findings = fixture_findings_with_allow(source, &["HOME_RESOLVER"]);
        assert_eq!(findings.len(), 1, "{findings:?}");
    }

    #[test]
    fn walk_covers_build_scripts_and_reincluded_files_and_skips_test_modules() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        let src = root.join("gatecrate/src");
        fs::create_dir_all(&src).expect("mkdir src");
        fs::create_dir_all(root.join("gatecrate/hidden")).expect("mkdir hidden");
        fs::write(
            src.join("lib.rs"),
            r#"
#[path = "../hidden/away.rs"]
mod away;
include!("../hidden/frag.rs");
#[cfg(test)]
#[path = "../hidden/testmod.rs"]
mod testmod;
"#,
        )
        .expect("write lib.rs");
        fs::write(
            root.join("gatecrate/hidden/away.rs"),
            "pub fn bad() -> Option<std::path::PathBuf> { dirs::home_dir() }\n",
        )
        .expect("write away.rs");
        fs::write(
            root.join("gatecrate/hidden/frag.rs"),
            "pub fn frag() -> Option<std::ffi::OsString> { std::env::var_os(\"HOME\") }\n",
        )
        .expect("write frag.rs");
        fs::write(
            root.join("gatecrate/hidden/testmod.rs"),
            "pub fn test_only() -> Option<std::path::PathBuf> { dirs::home_dir() }\n",
        )
        .expect("write testmod.rs");
        fs::write(
            root.join("gatecrate/build.rs"),
            "fn main() {\n    let _ = std::env::var_os(\"HOME\");\n}\n",
        )
        .expect("write build.rs");
        fs::write(
            src.join("machine_tests.rs"),
            "pub fn t() -> Option<std::path::PathBuf> { dirs::home_dir() }\n",
        )
        .expect("write machine_tests.rs");
        fs::write(
            src.join("layout.rs"),
            "pub fn authority() -> Option<std::path::PathBuf> { dirs::home_dir() }\n",
        )
        .expect("write layout.rs");
        let allowlist: [AllowEntry; 1] = [(
            "gatecrate/src/layout.rs",
            AllowScope::WholeFile,
            "fixture layout authority",
        )];
        let findings =
            collect_findings_for_roots(root, &["gatecrate/src"], &allowlist).expect("scan");
        let joined = findings.join("\n");
        assert!(joined.contains("hidden/away.rs"), "{joined}");
        assert!(joined.contains("hidden/frag.rs"), "{joined}");
        assert!(joined.contains("gatecrate/build.rs"), "{joined}");
        assert!(!joined.contains("testmod.rs"), "{joined}");
        assert!(!joined.contains("machine_tests.rs"), "{joined}");
        assert!(!joined.contains("layout.rs"), "{joined}");
        assert_eq!(findings.len(), 3, "{joined}");
    }
}
