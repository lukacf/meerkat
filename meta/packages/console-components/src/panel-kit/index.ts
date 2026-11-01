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
export { normalizeStatusWord, priorityTone, riskTierWord, riskTone, statusTone, toneWord } from "./tone";
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
export { Button, IconButton } from "./button";
export type { ButtonProps, ButtonSize, ButtonVariant, IconButtonProps } from "./button";
export { Chip, Count, StatusDot, StatusPill } from "./chip";
export type { ChipVariant } from "./chip";
export { StatusMark } from "./status-mark";
export type { StatusMarkSize } from "./status-mark";
export { KIT_GLYPH_NAMES, KitGlyph } from "./glyphs";
export type { KitGlyphName } from "./glyphs";
export { KeyValueList } from "./key-value";
export type { KeyValueItem } from "./key-value";
export { ListRow, RowList } from "./list-row";
export { Meter } from "./meter";
export type { MeterSegment } from "./meter";
export {
  PanelBody,
  PanelHeader,
  PanelSection,
  PanelShell,
  PanelTabPanel,
  PanelTabs,
  SectionHeader,
  StatusRow,
  StatusToast,
  panelTabIds,
  usePanelOptions,
} from "./panel";
export type {
  PanelDensity,
  PanelFreshness,
  PanelHeaderMenu,
  PanelOptions,
  PanelTab,
  PanelViewSwitch,
  SummaryFact,
} from "./panel";
export {
  PanelDensityDefaultProvider,
  PanelPreferencesProvider,
  panelPreferenceKey,
  standardPanelMenu,
  usePanelDensity,
  usePanelPreference,
  useShowTechnicalIds,
} from "./preferences";
export { RelativeTime, RelativeTimeChip, useNow } from "./relative-time";
export { Countdown, countdownTone, formatCountdown } from "./countdown";
export { EntityCard, EntityRow, TrustLadder } from "./entity-card";
export type { EntityCardProps, EntityKind, EntityNow, EntityTrend } from "./entity-card";
export { FilterBar, FilterChip } from "./filter-bar";
export type { FilterBarSearch, FilterChipProps } from "./filter-bar";
export { ConsoleNavigationProvider, EntityLink, useConsoleNavigation, wantsAlternate } from "./navigation";
export type { ConsoleNavigation, EntityLinkKind } from "./navigation";
export { NewItemsPill } from "./live";
export { SplitDetail } from "./split-detail";
export { EmptyState, LoadingState, Notice } from "./states";
export type { EmptyStateKind, LoadingVariant } from "./states";
export { EmptyIllustration } from "./empty-illustration";
export type { IllustrationName } from "./empty-illustration";
export { DetailDrawer, DrawerLayout, DrawerSection } from "./drawer";
export { StatTile, TileGrid } from "./stat-tile";
export { TECHNICAL_ID_TAIL, TechnicalId, sentenceFragment, splitTechnicalId } from "./technical-id";
export { TechnicalDetails, technicalDetailsText } from "./technical-details";
export type { TechnicalDetail } from "./technical-details";
export { SearchField, SegmentedControl, SelectField, Toolbar } from "./toolbar";
export type { SegmentOption, SelectOption } from "./toolbar";
export { Popover } from "./popover";
export type { PopoverCloseReason, PopoverPlacement, PopoverProps } from "./popover";
export { OverflowMenu } from "./menu";
export type {
  MenuCheckboxEntry,
  MenuCommandEntry,
  MenuEntry,
  MenuRadioEntry,
  MenuRadioOption,
  MenuSeparatorEntry,
  MenuTriggerProps,
} from "./menu";
export { ViewOptions, ViewOptionsMenu } from "./view-options";
export type { ViewOptionsControl, ViewOptionsProps } from "./view-options";
export { GlanceStrip } from "./glance-strip";
export { FigureTile, deltaTone, formatDelta, formatFigure } from "./figure-tile";
export type { FigureDelta } from "./figure-tile";
export { ChartTooltip } from "./chart-tooltip";

// Micro charts (spec A3.8) live in ../viz/charts; the kit re-exports them.
export { Sparkline } from "../viz/charts/sparkline";
export type { SparkPoint, SparkValues } from "../viz/charts/sparkline";
export { ActivityBars } from "../viz/charts/activity-bars";
export { MiniBar } from "../viz/charts/mini-bar";
export { BarcodeLegend, BarcodeMeter, barcodeUnitsPerSegment } from "../viz/charts/barcode-meter";
export type { BarcodeLabels } from "../viz/charts/barcode-meter";
export { HeatStrip, heatLevels } from "../viz/charts/heat-strip";
export type { HeatRow } from "../viz/charts/heat-strip";
export { ProgressRing, progressWords } from "../viz/charts/progress-ring";
export { DotStrip } from "../viz/charts/dot-strip";
export type { DotShape, DotStripItem } from "../viz/charts/dot-strip";
export {
  binCounts,
  binIndex,
  binTimes,
  brushRange,
  createTimeScale,
  formatTimeRange,
  formatTimeTick,
  niceTimeStep,
  nowMarker,
  placeAxisLabel,
  placeTickLabels,
  timeTicks,
} from "../viz/charts/time-axis";
export type { AxisLabelSpan, PlacedTickLabel, TimeScale, TimeTick, TimeTickFormat, TimeTickOptions } from "../viz/charts/time-axis";

// The dev-only kit gallery's section shape (type only: no gallery code ships).
export type { GallerySection } from "./gallery/types";
