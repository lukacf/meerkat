// The panel kit: the shared building blocks every console panel is built
// from (design brief 3.6). Styles live in ../styles/panel-kit.css.
//
// The pure modules (format, tone, agents) have no imports, so a panel whose
// test bundle cannot resolve React can import them directly by file.

export { cx } from "./cx";
export {
  formatAbsoluteTime,
  formatClockTime,
  formatDuration,
  formatRelativeTime,
  formatShortDate,
  humanizeEnum,
  joinMeta,
  plural,
  toEpochMs,
} from "./format";
export type { TimeInput } from "./format";
export { normalizeStatusWord, priorityTone, riskTierWord, riskTone, statusTone } from "./tone";
export type { Tone } from "./tone";
export {
  EMPTY_AGENT_DIRECTORY,
  agentDisplayName,
  agentInitial,
  createAgentDirectory,
  stripAgentKeyPrefix,
} from "./agents";
export type { AgentDirectory, AgentDirectoryEntry } from "./agents";
export { AgentAvatar, AgentDirectoryProvider, AgentName, useAgentDirectory } from "./agent-name";
export type { AgentAvatarSize } from "./agent-name";
export { Button } from "./button";
export type { ButtonProps, ButtonSize, ButtonVariant } from "./button";
export { Chip, Count, StatusDot } from "./chip";
export { KitGlyph } from "./glyphs";
export type { KitGlyphName } from "./glyphs";
export { KeyValueList } from "./key-value";
export type { KeyValueItem } from "./key-value";
export { ListRow, RowList } from "./list-row";
export { Meter } from "./meter";
export type { MeterSegment } from "./meter";
export { PanelBody, PanelHeader, PanelSection, PanelShell, PanelTabs, SectionHeader, StatusRow } from "./panel";
export type { PanelTab } from "./panel";
export { RelativeTime, useNow } from "./relative-time";
export { SplitDetail } from "./split-detail";
export { EmptyState, LoadingState, Notice } from "./states";
export type { EmptyStateKind } from "./states";
export { StatTile, TileGrid } from "./stat-tile";
export { TECHNICAL_ID_TAIL, TechnicalId, sentenceFragment, splitTechnicalId } from "./technical-id";
export { TechnicalDetails } from "./technical-details";
export type { TechnicalDetail } from "./technical-details";
export { SearchField, SegmentedControl, SelectField, Toolbar } from "./toolbar";
export type { SegmentOption, SelectOption } from "./toolbar";
