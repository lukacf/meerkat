//! Console UI configuration loaded from application config.
//!
//! This is intentionally a view-level contract. Agent ownership and runtime
//! routing still live in the mob/runtime layers; this config controls how the
//! stock console chooses sidebar affordances and groups already-projected
//! agents.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleUiConfig {
    /// Trusted same-origin JavaScript modules exporting custom panel arrays.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub panel_modules: Vec<String>,
    /// Dedicated MCP Apps sandbox URL, on an origin without Console APIs or cookies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_apps_sandbox_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "ConsoleBrandingConfig::is_default")]
    pub brand: ConsoleBrandingConfig,
    #[serde(default, skip_serializing_if = "ConsoleAppearanceConfig::is_default")]
    pub appearance: ConsoleAppearanceConfig,
    #[serde(default, skip_serializing_if = "ConsoleEnvironmentConfig::is_default")]
    pub environment: ConsoleEnvironmentConfig,
    #[serde(default, skip_serializing_if = "ConsoleLayoutConfig::is_default")]
    pub layout: ConsoleLayoutConfig,
    #[serde(default, skip_serializing_if = "ConsoleRailUiConfig::is_default")]
    pub rail: ConsoleRailUiConfig,
    #[serde(default, skip_serializing_if = "ConsoleSidebarUiConfig::is_default")]
    pub sidebar: ConsoleSidebarUiConfig,
    #[serde(default, skip_serializing_if = "ConsoleAgentListConfig::is_default")]
    pub agent_list: ConsoleAgentListConfig,
    #[serde(default, skip_serializing_if = "ConsoleActionsUiConfig::is_default")]
    pub actions: ConsoleActionsUiConfig,
}

impl ConsoleUiConfig {
    pub fn is_default(value: &Self) -> bool {
        value == &Self::default()
    }

    /// Validate before advertising MCP Apps to connected servers. A gateway
    /// supplies its actual bound or declared public origin once it knows it.
    pub fn validate_mcp_apps_sandbox(
        &self,
        console_url: Option<&str>,
    ) -> Result<(), ConsoleConfigError> {
        let Some(value) = &self.mcp_apps_sandbox_url else {
            return Ok(());
        };
        let invalid = || {
            ConsoleConfigError::Invalid(
                "mcp_apps_sandbox_url requires an absolute HTTP(S) URL \
                 without credentials or a fragment"
                    .into(),
            )
        };
        let sandbox = reqwest::Url::parse(value).map_err(|_| invalid())?;
        if !matches!(sandbox.scheme(), "http" | "https")
            || sandbox.host_str().is_none()
            || !sandbox.username().is_empty()
            || sandbox.password().is_some()
            || sandbox.fragment().is_some()
            || value.chars().any(char::is_whitespace)
        {
            return Err(invalid());
        }
        if let Some(console_url) = console_url {
            let console = reqwest::Url::parse(console_url).map_err(|_| {
                ConsoleConfigError::Invalid("Console origin is not an absolute URL".into())
            })?;
            if sandbox.origin() == console.origin() {
                return Err(ConsoleConfigError::Invalid(
                    "mcp_apps_sandbox_url must use an origin separate from the Console".into(),
                ));
            }
        }
        Ok(())
    }

    pub fn normalized(mut self) -> Self {
        self.panel_modules = normalize_string_vec(self.panel_modules);
        self.mcp_apps_sandbox_url = normalize_optional_string(self.mcp_apps_sandbox_url);
        self.title = normalize_optional_string(self.title);
        self.brand = self.brand.normalized();
        self.appearance = self.appearance.normalized();
        self.environment = self.environment.normalized();
        self.layout = self.layout.normalized();
        self.rail = self.rail.normalized();
        self.sidebar = self.sidebar.normalized();
        self.agent_list = self.agent_list.normalized();
        self.actions = self.actions.normalized();
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleBrandingConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo_alt: Option<String>,
}

impl ConsoleBrandingConfig {
    pub fn is_default(value: &Self) -> bool {
        value == &Self::default()
    }

    fn normalized(mut self) -> Self {
        self.label = normalize_optional_string(self.label);
        self.logo_url = normalize_optional_string(self.logo_url);
        self.logo_alt = normalize_optional_string(self.logo_alt);
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleAppearanceConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_theme: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_variant: Option<String>,
}

impl ConsoleAppearanceConfig {
    pub fn is_default(value: &Self) -> bool {
        value == &Self::default()
    }

    fn normalized(mut self) -> Self {
        self.default_theme = normalize_optional_string(self.default_theme);
        self.default_variant = normalize_optional_string(self.default_variant);
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleEnvironmentConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl ConsoleEnvironmentConfig {
    pub fn is_default(value: &Self) -> bool {
        value == &Self::default()
    }

    fn normalized(mut self) -> Self {
        self.label = normalize_optional_string(self.label);
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleLayoutConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_preset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_control: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar_collapsed: Option<bool>,
}

impl ConsoleLayoutConfig {
    pub fn is_default(value: &Self) -> bool {
        value == &Self::default()
    }

    fn normalized(mut self) -> Self {
        self.initial_preset = normalize_optional_string(self.initial_preset);
        self.initial_control = normalize_optional_string(self.initial_control);
        self.initial_agent = normalize_optional_string(self.initial_agent);
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleRailUiConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collapsed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_preset_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub empty_text: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filter_presets: Vec<ConsoleRailFilterPresetConfig>,
}

impl ConsoleRailUiConfig {
    pub fn is_default(value: &Self) -> bool {
        value == &Self::default()
    }

    fn normalized(mut self) -> Self {
        self.active_preset_id = normalize_optional_string(self.active_preset_id);
        self.empty_text = normalize_optional_string(self.empty_text);
        self.filter_presets = self
            .filter_presets
            .into_iter()
            .map(ConsoleRailFilterPresetConfig::normalized)
            .filter(|preset| !preset.id.is_empty() && !preset.label.is_empty())
            .collect();
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleRailFilterPresetConfig {
    pub id: String,
    pub label: String,
    #[serde(
        default,
        rename = "watchedOnly",
        alias = "watched_only",
        skip_serializing_if = "Option::is_none"
    )]
    pub watched_only: Option<bool>,
    #[serde(
        default,
        rename = "alertLevels",
        alias = "alert_levels",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub alert_levels: Vec<String>,
}

impl ConsoleRailFilterPresetConfig {
    fn normalized(mut self) -> Self {
        self.id = self.id.trim().to_string();
        self.label = self.label.trim().to_string();
        self.alert_levels = normalize_string_vec(self.alert_levels);
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleSidebarUiConfig {
    /// If present, only these stock workbench controls are visible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_controls: Option<Vec<String>>,
    /// Stock workbench controls to hide when `visible_controls` is absent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden_controls: Vec<String>,
    /// Extra sidebar buttons. Buttons can either open a stock `control` or
    /// link to an `href`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buttons: Vec<ConsoleSidebarButtonConfig>,
}

impl ConsoleSidebarUiConfig {
    pub fn is_default(value: &Self) -> bool {
        value == &Self::default()
    }

    fn normalized(mut self) -> Self {
        self.visible_controls = self.visible_controls.map(normalize_string_vec);
        self.hidden_controls = normalize_string_vec(self.hidden_controls);
        self.buttons = self
            .buttons
            .into_iter()
            .map(ConsoleSidebarButtonConfig::normalized)
            .filter(ConsoleSidebarButtonConfig::is_valid)
            .collect();
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleSidebarButtonConfig {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    /// A same-origin path the console opens as a docked operator panel. The
    /// panel is trusted code: it runs with the viewing operator's console
    /// session and authority. It comes only from static operator
    /// configuration and excludes `control` and `href`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, alias = "iconName", skip_serializing_if = "Option::is_none")]
    pub icon_name: Option<String>,
}

impl ConsoleSidebarButtonConfig {
    fn normalized(mut self) -> Self {
        self.id = self.id.trim().to_string();
        self.label = self.label.trim().to_string();
        self.control = normalize_optional_string(self.control);
        self.href = normalize_optional_string(self.href);
        // A panel path is validated as written: surrounding whitespace is
        // refused, not trimmed. An empty panel means no panel, like the
        // other optional strings, so a button with a valid href or control
        // keeps working.
        self.panel = self.panel.filter(|panel| !panel.is_empty());
        self.target = normalize_optional_string(self.target);
        self.icon_name = normalize_optional_string(self.icon_name);
        self
    }

    fn is_valid(&self) -> bool {
        if self.id.is_empty() || self.label.is_empty() {
            return false;
        }
        if let Some(panel) = &self.panel {
            return self.control.is_none()
                && self.href.is_none()
                && valid_operator_panel_path(panel);
        }
        self.control.as_ref().is_some_and(|value| !value.is_empty())
            || self.href.as_ref().is_some_and(|value| !value.is_empty())
    }
}

/// A docked operator panel must be a same-origin path: one leading `/`, at
/// most 2048 UTF-8 bytes, no whitespace, control characters or backslashes,
/// and no `.` or `..` segment in any spelling (`%2e` included). The path is
/// used as written; it is never normalized and reparsed.
fn valid_operator_panel_path(path: &str) -> bool {
    path.starts_with('/')
        && !path.starts_with("//")
        && path.len() <= 2048
        && !path.chars().any(|character| {
            character.is_whitespace()
                || character.is_control()
                || character == '\\'
                // The console refuses a byte order mark as whitespace too.
                || character == '\u{feff}'
        })
        && !has_dot_segment(path)
}

/// Whether the path part (before `?` or `#`) has a `.` or `..` segment,
/// counting percent-encoded dots.
fn has_dot_segment(path: &str) -> bool {
    let path = path.split(['?', '#']).next().unwrap_or(path);
    path.split('/').any(|segment| {
        let decoded = segment.to_ascii_lowercase().replace("%2e", ".");
        decoded == "." || decoded == ".."
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleAgentListConfig {
    /// Selectors tried in order for the primary section. Supported selectors
    /// include `group`, `role`, `kind`, and `labels.<key>`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub group_by: Vec<String>,
    /// Selectors tried in order for an optional section-local subgroup.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subgroup_by: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub section_order: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_subgroup: Option<String>,
    /// Defaults to true in the console: if a section only has one subgroup,
    /// the subgroup header is suppressed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collapse_single_subgroup: Option<bool>,
    /// Optional default pinned agents for hosts that want a starter view.
    /// User localStorage preferences take precedence once present.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_pinned_agent_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub badges: Vec<ConsoleAgentBadgeConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<ConsoleAgentSectionConfig>,
}

impl ConsoleAgentListConfig {
    pub fn is_default(value: &Self) -> bool {
        value == &Self::default()
    }

    fn normalized(mut self) -> Self {
        self.group_by = normalize_string_vec(self.group_by);
        self.subgroup_by = normalize_string_vec(self.subgroup_by);
        self.section_order = normalize_string_vec(self.section_order);
        self.fallback_group = normalize_optional_string(self.fallback_group);
        self.fallback_subgroup = normalize_optional_string(self.fallback_subgroup);
        self.default_pinned_agent_ids = normalize_string_vec(self.default_pinned_agent_ids);
        self.badges = self
            .badges
            .into_iter()
            .map(ConsoleAgentBadgeConfig::normalized)
            .filter(ConsoleAgentBadgeConfig::is_valid)
            .collect();
        self.sections = self
            .sections
            .into_iter()
            .map(ConsoleAgentSectionConfig::normalized)
            .filter(|section| !section.name.is_empty())
            .collect();
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleAgentBadgeConfig {
    pub id: String,
    pub label: String,
    pub field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<String>,
}

impl ConsoleAgentBadgeConfig {
    fn normalized(mut self) -> Self {
        self.id = self.id.trim().to_string();
        self.label = self.label.trim().to_string();
        self.field = self.field.trim().to_string();
        self.tone = normalize_optional_string(self.tone);
        self
    }

    fn is_valid(&self) -> bool {
        !self.id.is_empty() && !self.label.is_empty() && !self.field.is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleAgentSectionConfig {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collapsed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub empty_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub empty_text: Option<String>,
}

impl ConsoleAgentSectionConfig {
    fn normalized(mut self) -> Self {
        self.name = self.name.trim().to_string();
        self.empty_title = normalize_optional_string(self.empty_title);
        self.empty_text = normalize_optional_string(self.empty_text);
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleActionsUiConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspect_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub respawn_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retire_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_inspect: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_chat: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_respawn: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_retire: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_reset: Option<bool>,
}

impl ConsoleActionsUiConfig {
    pub fn is_default(value: &Self) -> bool {
        value == &Self::default()
    }

    fn normalized(mut self) -> Self {
        self.inspect_label = normalize_optional_string(self.inspect_label);
        self.chat_label = normalize_optional_string(self.chat_label);
        self.send_label = normalize_optional_string(self.send_label);
        self.respawn_label = normalize_optional_string(self.respawn_label);
        self.retire_label = normalize_optional_string(self.retire_label);
        self.reset_label = normalize_optional_string(self.reset_label);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsoleConfigError {
    Io(String),
    TomlParse(String),
    Invalid(String),
}

impl std::fmt::Display for ConsoleConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(message) => write!(f, "I/O error: {message}"),
            Self::TomlParse(message) => write!(f, "TOML parse error: {message}"),
            Self::Invalid(message) => write!(f, "invalid Console configuration: {message}"),
        }
    }
}

impl std::error::Error for ConsoleConfigError {}

#[derive(Debug, Clone, Default, Deserialize)]
struct ConsoleUiConfigPatch {
    #[serde(default)]
    panel_modules: Option<Vec<String>>,
    #[serde(default)]
    mcp_apps_sandbox_url: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    brand: Option<ConsoleBrandingConfigPatch>,
    #[serde(default)]
    appearance: Option<ConsoleAppearanceConfigPatch>,
    #[serde(default)]
    environment: Option<ConsoleEnvironmentConfigPatch>,
    #[serde(default)]
    layout: Option<ConsoleLayoutConfigPatch>,
    #[serde(default)]
    rail: Option<ConsoleRailUiConfigPatch>,
    #[serde(default)]
    sidebar: Option<ConsoleSidebarUiConfigPatch>,
    #[serde(default)]
    agent_list: Option<ConsoleAgentListConfigPatch>,
    #[serde(default)]
    actions: Option<ConsoleActionsUiConfigPatch>,
    #[serde(default)]
    realms: BTreeMap<String, ConsoleUiConfigPatch>,
}

impl ConsoleUiConfigPatch {
    fn apply_to(&self, config: &mut ConsoleUiConfig) {
        if let Some(modules) = &self.panel_modules {
            config.panel_modules = normalize_string_vec(modules.clone());
        }
        if let Some(url) = &self.mcp_apps_sandbox_url {
            config.mcp_apps_sandbox_url = normalize_optional_string(Some(url.clone()));
        }
        if let Some(title) = &self.title {
            config.title = normalize_optional_string(Some(title.clone()));
        }
        if let Some(brand) = &self.brand {
            brand.apply_to(&mut config.brand);
        }
        if let Some(appearance) = &self.appearance {
            appearance.apply_to(&mut config.appearance);
        }
        if let Some(environment) = &self.environment {
            environment.apply_to(&mut config.environment);
        }
        if let Some(layout) = &self.layout {
            layout.apply_to(&mut config.layout);
        }
        if let Some(rail) = &self.rail {
            rail.apply_to(&mut config.rail);
        }
        if let Some(sidebar) = &self.sidebar {
            sidebar.apply_to(&mut config.sidebar);
        }
        if let Some(agent_list) = &self.agent_list {
            agent_list.apply_to(&mut config.agent_list);
        }
        if let Some(actions) = &self.actions {
            actions.apply_to(&mut config.actions);
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ConsoleBrandingConfigPatch {
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    logo_url: Option<String>,
    #[serde(default)]
    logo_alt: Option<String>,
}

impl ConsoleBrandingConfigPatch {
    fn apply_to(&self, config: &mut ConsoleBrandingConfig) {
        if let Some(label) = &self.label {
            config.label = normalize_optional_string(Some(label.clone()));
        }
        if let Some(logo_url) = &self.logo_url {
            config.logo_url = normalize_optional_string(Some(logo_url.clone()));
        }
        if let Some(logo_alt) = &self.logo_alt {
            config.logo_alt = normalize_optional_string(Some(logo_alt.clone()));
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ConsoleAppearanceConfigPatch {
    #[serde(default)]
    default_theme: Option<String>,
    #[serde(default)]
    default_variant: Option<String>,
}

impl ConsoleAppearanceConfigPatch {
    fn apply_to(&self, config: &mut ConsoleAppearanceConfig) {
        if let Some(default_theme) = &self.default_theme {
            config.default_theme = normalize_optional_string(Some(default_theme.clone()));
        }
        if let Some(default_variant) = &self.default_variant {
            config.default_variant = normalize_optional_string(Some(default_variant.clone()));
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ConsoleEnvironmentConfigPatch {
    #[serde(default)]
    label: Option<String>,
}

impl ConsoleEnvironmentConfigPatch {
    fn apply_to(&self, config: &mut ConsoleEnvironmentConfig) {
        if let Some(label) = &self.label {
            config.label = normalize_optional_string(Some(label.clone()));
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ConsoleLayoutConfigPatch {
    #[serde(default)]
    initial_preset: Option<String>,
    #[serde(default)]
    initial_control: Option<String>,
    #[serde(default)]
    initial_agent: Option<String>,
    #[serde(default)]
    sidebar_collapsed: Option<bool>,
}

impl ConsoleLayoutConfigPatch {
    fn apply_to(&self, config: &mut ConsoleLayoutConfig) {
        if let Some(initial_preset) = &self.initial_preset {
            config.initial_preset = normalize_optional_string(Some(initial_preset.clone()));
        }
        if let Some(initial_control) = &self.initial_control {
            config.initial_control = normalize_optional_string(Some(initial_control.clone()));
        }
        if let Some(initial_agent) = &self.initial_agent {
            config.initial_agent = normalize_optional_string(Some(initial_agent.clone()));
        }
        if let Some(sidebar_collapsed) = self.sidebar_collapsed {
            config.sidebar_collapsed = Some(sidebar_collapsed);
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ConsoleRailUiConfigPatch {
    #[serde(default)]
    visible: Option<bool>,
    #[serde(default)]
    collapsed: Option<bool>,
    #[serde(default)]
    active_preset_id: Option<String>,
    #[serde(default)]
    empty_text: Option<String>,
    #[serde(default)]
    filter_presets: Option<Vec<ConsoleRailFilterPresetConfig>>,
}

impl ConsoleRailUiConfigPatch {
    fn apply_to(&self, config: &mut ConsoleRailUiConfig) {
        if let Some(visible) = self.visible {
            config.visible = Some(visible);
        }
        if let Some(collapsed) = self.collapsed {
            config.collapsed = Some(collapsed);
        }
        if let Some(active_preset_id) = &self.active_preset_id {
            config.active_preset_id = normalize_optional_string(Some(active_preset_id.clone()));
        }
        if let Some(empty_text) = &self.empty_text {
            config.empty_text = normalize_optional_string(Some(empty_text.clone()));
        }
        if let Some(filter_presets) = &self.filter_presets {
            config.filter_presets = filter_presets
                .iter()
                .cloned()
                .map(ConsoleRailFilterPresetConfig::normalized)
                .filter(|preset| !preset.id.is_empty() && !preset.label.is_empty())
                .collect();
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ConsoleSidebarUiConfigPatch {
    #[serde(default)]
    visible_controls: Option<Vec<String>>,
    #[serde(default)]
    hidden_controls: Option<Vec<String>>,
    #[serde(default)]
    buttons: Option<Vec<ConsoleSidebarButtonConfig>>,
}

impl ConsoleSidebarUiConfigPatch {
    fn apply_to(&self, config: &mut ConsoleSidebarUiConfig) {
        if let Some(visible_controls) = &self.visible_controls {
            config.visible_controls = Some(normalize_string_vec(visible_controls.clone()));
        }
        if let Some(hidden_controls) = &self.hidden_controls {
            config.hidden_controls = normalize_string_vec(hidden_controls.clone());
        }
        if let Some(buttons) = &self.buttons {
            config.buttons = buttons
                .iter()
                .cloned()
                .map(ConsoleSidebarButtonConfig::normalized)
                .filter(ConsoleSidebarButtonConfig::is_valid)
                .collect();
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ConsoleAgentListConfigPatch {
    #[serde(default)]
    group_by: Option<Vec<String>>,
    #[serde(default)]
    subgroup_by: Option<Vec<String>>,
    #[serde(default)]
    section_order: Option<Vec<String>>,
    #[serde(default)]
    fallback_group: Option<String>,
    #[serde(default)]
    fallback_subgroup: Option<String>,
    #[serde(default)]
    collapse_single_subgroup: Option<bool>,
    #[serde(default)]
    default_pinned_agent_ids: Option<Vec<String>>,
    #[serde(default)]
    badges: Option<Vec<ConsoleAgentBadgeConfig>>,
    #[serde(default)]
    sections: Option<Vec<ConsoleAgentSectionConfig>>,
}

impl ConsoleAgentListConfigPatch {
    fn apply_to(&self, config: &mut ConsoleAgentListConfig) {
        if let Some(group_by) = &self.group_by {
            config.group_by = normalize_string_vec(group_by.clone());
        }
        if let Some(subgroup_by) = &self.subgroup_by {
            config.subgroup_by = normalize_string_vec(subgroup_by.clone());
        }
        if let Some(section_order) = &self.section_order {
            config.section_order = normalize_string_vec(section_order.clone());
        }
        if let Some(fallback_group) = &self.fallback_group {
            config.fallback_group = normalize_optional_string(Some(fallback_group.clone()));
        }
        if let Some(fallback_subgroup) = &self.fallback_subgroup {
            config.fallback_subgroup = normalize_optional_string(Some(fallback_subgroup.clone()));
        }
        if let Some(collapse_single_subgroup) = self.collapse_single_subgroup {
            config.collapse_single_subgroup = Some(collapse_single_subgroup);
        }
        if let Some(default_pinned_agent_ids) = &self.default_pinned_agent_ids {
            config.default_pinned_agent_ids =
                normalize_string_vec(default_pinned_agent_ids.clone());
        }
        if let Some(badges) = &self.badges {
            config.badges = badges
                .iter()
                .cloned()
                .map(ConsoleAgentBadgeConfig::normalized)
                .filter(ConsoleAgentBadgeConfig::is_valid)
                .collect();
        }
        if let Some(sections) = &self.sections {
            config.sections = sections
                .iter()
                .cloned()
                .map(ConsoleAgentSectionConfig::normalized)
                .filter(|section| !section.name.is_empty())
                .collect();
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ConsoleActionsUiConfigPatch {
    #[serde(default)]
    inspect_label: Option<String>,
    #[serde(default)]
    chat_label: Option<String>,
    #[serde(default)]
    send_label: Option<String>,
    #[serde(default)]
    respawn_label: Option<String>,
    #[serde(default)]
    retire_label: Option<String>,
    #[serde(default)]
    reset_label: Option<String>,
    #[serde(default)]
    show_inspect: Option<bool>,
    #[serde(default)]
    show_chat: Option<bool>,
    #[serde(default)]
    show_respawn: Option<bool>,
    #[serde(default)]
    show_retire: Option<bool>,
    #[serde(default)]
    show_reset: Option<bool>,
}

impl ConsoleActionsUiConfigPatch {
    fn apply_to(&self, config: &mut ConsoleActionsUiConfig) {
        if let Some(inspect_label) = &self.inspect_label {
            config.inspect_label = normalize_optional_string(Some(inspect_label.clone()));
        }
        if let Some(chat_label) = &self.chat_label {
            config.chat_label = normalize_optional_string(Some(chat_label.clone()));
        }
        if let Some(send_label) = &self.send_label {
            config.send_label = normalize_optional_string(Some(send_label.clone()));
        }
        if let Some(respawn_label) = &self.respawn_label {
            config.respawn_label = normalize_optional_string(Some(respawn_label.clone()));
        }
        if let Some(retire_label) = &self.retire_label {
            config.retire_label = normalize_optional_string(Some(retire_label.clone()));
        }
        if let Some(reset_label) = &self.reset_label {
            config.reset_label = normalize_optional_string(Some(reset_label.clone()));
        }
        if let Some(show_inspect) = self.show_inspect {
            config.show_inspect = Some(show_inspect);
        }
        if let Some(show_chat) = self.show_chat {
            config.show_chat = Some(show_chat);
        }
        if let Some(show_respawn) = self.show_respawn {
            config.show_respawn = Some(show_respawn);
        }
        if let Some(show_retire) = self.show_retire {
            config.show_retire = Some(show_retire);
        }
        if let Some(show_reset) = self.show_reset {
            config.show_reset = Some(show_reset);
        }
    }
}

pub fn load_console_ui_config_from_toml(
    toml_text: &str,
) -> Result<ConsoleUiConfig, ConsoleConfigError> {
    load_console_ui_config_from_toml_for_realm(toml_text, None)
}

pub fn load_console_ui_config_from_toml_for_realm(
    toml_text: &str,
    realm: Option<&str>,
) -> Result<ConsoleUiConfig, ConsoleConfigError> {
    let patch: ConsoleUiConfigPatch =
        toml::from_str(toml_text).map_err(|err| ConsoleConfigError::TomlParse(err.to_string()))?;
    let mut config = ConsoleUiConfig::default();
    patch.apply_to(&mut config);
    if let Some(realm) = realm.map(str::trim).filter(|value| !value.is_empty())
        && let Some(overlay) = patch.realms.get(realm)
    {
        overlay.apply_to(&mut config);
    }
    let config = config.normalized();
    config.validate_mcp_apps_sandbox(None)?;
    Ok(config)
}

pub fn load_console_ui_config_from_path_for_realm(
    path: impl AsRef<Path>,
    realm: Option<&str>,
) -> Result<ConsoleUiConfig, ConsoleConfigError> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path).map_err(|err| {
        ConsoleConfigError::Io(format!("failed to read {}: {err}", path.display()))
    })?;
    load_console_ui_config_from_toml_for_realm(&text, realm)
}

fn normalize_string_vec(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_panel_buttons_need_one_same_origin_path() -> Result<(), Box<dyn std::error::Error>>
    {
        let button = |panel: &str, href: Option<&str>| ConsoleSidebarButtonConfig {
            id: "reviews".to_string(),
            label: "Reviews".to_string(),
            panel: Some(panel.to_string()),
            href: href.map(str::to_string),
            ..ConsoleSidebarButtonConfig::default()
        };
        assert!(
            button("/host/reviews?tab=open", None)
                .normalized()
                .is_valid()
        );
        let empty_panel = ConsoleSidebarButtonConfig {
            id: "docs".to_string(),
            label: "Docs".to_string(),
            href: Some("https://docs.example/".to_string()),
            panel: Some(String::new()),
            ..ConsoleSidebarButtonConfig::default()
        }
        .normalized();
        assert_eq!(empty_panel.panel, None);
        assert!(empty_panel.is_valid(), "an empty panel is no panel");
        for path in [
            "https://host.example/reviews",
            "//host.example/reviews",
            "host/reviews",
            "/host reviews",
            " /host/reviews",
            "/host/reviews ",
            "/host\\reviews",
            "/host\u{7}reviews",
            "/host\u{85}reviews",
            "\u{feff}/host/reviews",
            "/host/\u{feff}reviews",
            "/.//evil.example/x",
            "/..//x",
            "/%2e//x",
            "/%2E%2e//evil.example/x",
            "/a/../b",
            "/a/./b",
        ] {
            assert!(!button(path, None).normalized().is_valid(), "{path:?}");
        }
        assert!(
            !button(&format!("/{}", "a".repeat(2048)), None)
                .normalized()
                .is_valid()
        );
        // The limit counts UTF-8 bytes, not characters.
        let multibyte = format!("/{}", "\u{e9}".repeat(1023));
        assert_eq!(multibyte.len(), 2047);
        assert!(button(&multibyte, None).normalized().is_valid());
        assert!(
            !button(&format!("{multibyte}\u{e9}"), None)
                .normalized()
                .is_valid()
        );
        assert!(
            button("/host/a.b/..c/reviews?next=/../", None)
                .normalized()
                .is_valid()
        );
        assert!(
            !button("/host/reviews", Some("https://host.example"))
                .normalized()
                .is_valid()
        );

        let config = load_console_ui_config_from_toml(
            "[[sidebar.buttons]]\nid = \"reviews\"\nlabel = \"Reviews\"\npanel = \"/host/reviews\"\n\n[[sidebar.buttons]]\nid = \"bad\"\nlabel = \"Bad\"\npanel = \"https://evil.example/\"\n",
        )?;
        assert_eq!(config.sidebar.buttons.len(), 1);
        assert_eq!(
            config.sidebar.buttons[0].panel.as_deref(),
            Some("/host/reviews")
        );
        Ok(())
    }

    /// The Console configuration Meta Forge compiles for an application on
    /// the Meta Toolkit Connect route ends with a "Connect setup" sidebar
    /// button that opens the host's Toolkit setup page as a docked operator
    /// panel. Read from disk as a gateway loads its Console configuration,
    /// with or without a realm, the button keeps its panel path through
    /// normalization and reaches the Console as written.
    #[test]
    fn connect_setup_panel_button_survives_normalization() -> Result<(), Box<dyn std::error::Error>>
    {
        // The file as the compiler writes it, the button table last.
        let compiled = r#"title = "Research desk"
[brand]
label = "Research desk"

[appearance]
default_theme = "dark"
default_variant = "graphite"

[environment]
label = "local"
[layout]
initial_preset = "two_columns"
initial_agent = "lead"

[agent_list]
group_by = ["labels.console_group"]
default_pinned_agent_ids = ["lead"]

[actions]
show_reset = false

[[sidebar.buttons]]
id = "connect-setup"
label = "Connect setup"
panel = "/host/toolkit/setup"
"#;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("console.toml");
        std::fs::write(&path, compiled)?;
        let connect_setup = ConsoleSidebarButtonConfig {
            id: "connect-setup".to_string(),
            label: "Connect setup".to_string(),
            panel: Some("/host/toolkit/setup".to_string()),
            ..ConsoleSidebarButtonConfig::default()
        };
        for realm in [None, Some("ops")] {
            let config = load_console_ui_config_from_path_for_realm(&path, realm)?;
            assert_eq!(
                config.sidebar.buttons,
                vec![connect_setup.clone()],
                "{realm:?}"
            );
            assert_eq!(config.clone().normalized(), config, "{realm:?}");
            assert_eq!(
                serde_json::to_value(&config)?["sidebar"],
                serde_json::json!({
                    "buttons": [{
                        "id": "connect-setup",
                        "label": "Connect setup",
                        "panel": "/host/toolkit/setup",
                    }],
                }),
                "{realm:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn mcp_apps_sandbox_rejects_invalid_urls_before_capability_bootstrap()
    -> Result<(), Box<dyn std::error::Error>> {
        for value in [
            "/sandbox.html",
            "//apps.example.test/sandbox.html",
            "file:///sandbox.html",
            "javascript:alert(1)",
            "https://user:private@apps.example.test/sandbox.html",
            "https://apps.example.test/sandbox.html#fragment",
            "https://apps.example.test/sandbox.html#",
        ] {
            let source = format!("mcp_apps_sandbox_url = {}", serde_json::json!(value));
            let error = load_console_ui_config_from_toml(&source)
                .err()
                .ok_or("invalid sandbox URL unexpectedly accepted")?;
            assert!(matches!(error, ConsoleConfigError::Invalid(_)));
            assert!(!error.to_string().contains("private"));
        }
        Ok(())
    }

    #[test]
    fn mcp_apps_sandbox_compares_only_actual_known_console_origins() {
        let config = ConsoleUiConfig {
            mcp_apps_sandbox_url: Some("https://console.example.test:443/sandbox.html".into()),
            ..Default::default()
        };
        assert!(config.validate_mcp_apps_sandbox(None).is_ok());
        assert!(
            config
                .validate_mcp_apps_sandbox(Some("https://console.example.test/console"))
                .is_err()
        );
        assert!(
            config
                .validate_mcp_apps_sandbox(Some("https://console.example.test:8443"))
                .is_ok()
        );
        assert!(
            config
                .validate_mcp_apps_sandbox(Some("http://127.0.0.1:5000"))
                .is_ok()
        );
        assert!(
            ConsoleUiConfig::default()
                .validate_mcp_apps_sandbox(Some("https://console.example.test"))
                .is_ok()
        );
    }

    #[test]
    fn mcp_apps_sandbox_is_projected_and_realm_overridable()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
mcp_apps_sandbox_url = " https://apps.example.test/sandbox.html "
[realms.demo]
mcp_apps_sandbox_url = "http://127.0.0.1:5001/sandbox.html"
[realms.disabled]
mcp_apps_sandbox_url = " "
"#;
        let base = load_console_ui_config_from_toml(source)?;
        assert_eq!(
            base.mcp_apps_sandbox_url.as_deref(),
            Some("https://apps.example.test/sandbox.html")
        );
        assert_eq!(
            serde_json::to_value(&base)?["mcp_apps_sandbox_url"],
            "https://apps.example.test/sandbox.html"
        );
        let demo = load_console_ui_config_from_toml_for_realm(source, Some("demo"))?;
        assert_eq!(
            demo.mcp_apps_sandbox_url.as_deref(),
            Some("http://127.0.0.1:5001/sandbox.html")
        );
        let disabled = load_console_ui_config_from_toml_for_realm(source, Some("disabled"))?;
        assert!(
            serde_json::to_value(disabled)?
                .get("mcp_apps_sandbox_url")
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn panel_modules_are_projected_and_realm_overridable() -> Result<(), Box<dyn std::error::Error>>
    {
        let source = r#"
panel_modules = [" /assets/base.js ", ""]
[realms.demo]
panel_modules = ["/assets/demo.js"]
[realms.disabled]
panel_modules = []
"#;
        let base = load_console_ui_config_from_toml(source)?;
        assert_eq!(base.panel_modules, vec!["/assets/base.js"]);
        assert_eq!(
            serde_json::to_value(&base)?["panel_modules"],
            serde_json::json!(["/assets/base.js"])
        );
        let demo = load_console_ui_config_from_toml_for_realm(source, Some("demo"))?;
        assert_eq!(demo.panel_modules, vec!["/assets/demo.js"]);
        let disabled = load_console_ui_config_from_toml_for_realm(source, Some("disabled"))?;
        assert!(disabled.panel_modules.is_empty());
        assert!(
            serde_json::to_value(disabled)?
                .get("panel_modules")
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn loads_console_toml_with_sidebar_buttons_and_agent_selectors()
    -> Result<(), ConsoleConfigError> {
        let config = load_console_ui_config_from_toml(
            r#"
title = "Ops"

[brand]
label = "Ops Console"
logo_url = "/assets/ops.svg"
logo_alt = "Ops"

[appearance]
default_theme = "dark"
default_variant = "graphite"

[environment]
label = "prod"

[layout]
initial_preset = "two_columns"
initial_control = "roster"
sidebar_collapsed = false

[rail]
visible = true
collapsed = false
active_preset_id = "critical"
empty_text = "No signals."

[[rail.filter_presets]]
id = "critical"
label = "Critical"
alert_levels = ["critical"]

[sidebar]
visible_controls = ["topology", "roster", "logs"]

[[sidebar.buttons]]
id = "ops"
label = "Ops"
href = "https://example.test/ops"
target = "_blank"

[agent_list]
group_by = ["labels.console_group", "labels.group", "role"]
subgroup_by = ["labels.org"]
section_order = ["Personal", "Initiatives", "Internal"]
fallback_group = "Other"
default_pinned_agent_ids = ["identity:ops-lead"]

[[agent_list.badges]]
id = "org"
label = "Org"
field = "labels.org"
tone = "info"

[[agent_list.sections]]
name = "Initiatives"
empty_title = "No initiatives"
empty_text = "Create one in Linear."

[actions]
inspect_label = "Profile"
chat_label = "Talk"
send_label = "Send to agent"
show_reset = false
"#,
        )?;

        assert_eq!(config.title.as_deref(), Some("Ops"));
        assert_eq!(config.brand.label.as_deref(), Some("Ops Console"));
        assert_eq!(config.brand.logo_url.as_deref(), Some("/assets/ops.svg"));
        assert_eq!(config.brand.logo_alt.as_deref(), Some("Ops"));
        assert_eq!(config.appearance.default_theme.as_deref(), Some("dark"));
        assert_eq!(config.environment.label.as_deref(), Some("prod"));
        assert_eq!(config.layout.initial_preset.as_deref(), Some("two_columns"));
        assert_eq!(config.rail.filter_presets[0].alert_levels, vec!["critical"]);
        assert_eq!(
            config.sidebar.visible_controls,
            Some(vec![
                "topology".to_string(),
                "roster".to_string(),
                "logs".to_string()
            ])
        );
        assert_eq!(config.sidebar.buttons.len(), 1);
        assert_eq!(config.agent_list.subgroup_by, vec!["labels.org"]);
        assert_eq!(
            config.agent_list.default_pinned_agent_ids,
            vec!["identity:ops-lead"]
        );
        assert_eq!(config.agent_list.badges[0].field, "labels.org");
        assert_eq!(config.agent_list.sections[0].name, "Initiatives");
        assert_eq!(config.actions.inspect_label.as_deref(), Some("Profile"));
        assert_eq!(config.actions.show_reset, Some(false));
        Ok(())
    }

    #[test]
    fn realm_overlay_replaces_only_configured_fields() -> Result<(), ConsoleConfigError> {
        let config = load_console_ui_config_from_toml_for_realm(
            r#"
title = "Default"

[sidebar]
visible_controls = ["topology", "roster"]

[agent_list]
group_by = ["labels.group"]

[realms.ops]
title = "Ops"

[realms.ops.brand]
label = "Ops"

[realms.ops.agent_list]
subgroup_by = ["labels.org"]

[realms.ops.layout]
initial_control = "logs"
"#,
            Some("ops"),
        )?;

        assert_eq!(config.title.as_deref(), Some("Ops"));
        assert_eq!(
            config.sidebar.visible_controls,
            Some(vec!["topology".to_string(), "roster".to_string()])
        );
        assert_eq!(config.brand.label.as_deref(), Some("Ops"));
        assert_eq!(config.layout.initial_control.as_deref(), Some("logs"));
        assert_eq!(config.agent_list.group_by, vec!["labels.group"]);
        assert_eq!(config.agent_list.subgroup_by, vec!["labels.org"]);
        Ok(())
    }
}
