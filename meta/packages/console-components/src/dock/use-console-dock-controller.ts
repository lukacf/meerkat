import {
  useMemo,
  useRef,
  useState,
  type Dispatch,
  type SetStateAction,
} from "react";

import {
  applyConsoleDockAction,
  buildConsoleDockViewState,
  consoleDockPresets,
  createConsoleDockState,
  type BuildConsoleDockViewStateOptions,
  type ConsoleDockAction,
  type ConsoleDockCreatePanelState,
  type ConsoleDockOpenIntent,
  type ConsoleDockPanelMode,
  type ConsoleDockPanelSplitDirection,
  type ConsoleDockPanelState,
  type ConsoleDockPreset,
  type ConsoleDockPresetId,
  type ConsoleDockState,
  type ConsoleDockSuggestTargets,
  type ConsoleDockTarget,
  type ConsoleDockViewState,
} from "@console-core";

export type ConsoleDockController<TTarget extends ConsoleDockTarget = ConsoleDockTarget> = {
  state: ConsoleDockState<TTarget>;
  setState: Dispatch<SetStateAction<ConsoleDockState<TTarget>>>;
  viewState: ConsoleDockViewState<TTarget>;
  presets: ConsoleDockPreset[];
  focusedPanel: ConsoleDockPanelState<TTarget> | null;
  focusedPanelId: string | null;
  focusedTarget: TTarget | null;
  dispatch: (action: ConsoleDockAction<TTarget>) => void;
  createTab: (options?: { blank?: boolean }) => void;
  selectTab: (tabId: string) => void;
  closeTab: (tabId: string) => void;
  focusPanel: (panelId: string) => void;
  closePanel: (panelId: string) => void;
  splitPanel: (panelId: string, direction: ConsoleDockPanelSplitDirection) => void;
  resizeSplit: (splitId: string, ratio: number) => void;
  applyPreset: (presetId: ConsoleDockPresetId) => void;
  openTarget: (target: TTarget, intent?: ConsoleDockOpenIntent) => void;
  setPanelTarget: (panelId: string, target: TTarget | null) => void;
  setPanelMode: (panelId: string, mode: ConsoleDockPanelMode) => void;
};

export type UseConsoleDockControllerOptions<TTarget extends ConsoleDockTarget = ConsoleDockTarget> = {
  initialTarget?: TTarget | null;
  initialPresetId?: ConsoleDockPresetId;
  createPanelState: ConsoleDockCreatePanelState<TTarget>;
  suggestTargets?: ConsoleDockSuggestTargets<TTarget>;
  resolvePanelView?: BuildConsoleDockViewStateOptions<TTarget>["resolvePanelView"];
  resolveTabView?: BuildConsoleDockViewStateOptions<TTarget>["resolveTabView"];
};

export function useConsoleDockController<TTarget extends ConsoleDockTarget = ConsoleDockTarget>({
  initialTarget = null,
  initialPresetId = "single",
  createPanelState,
  suggestTargets,
  resolvePanelView,
  resolveTabView,
}: UseConsoleDockControllerOptions<TTarget>): ConsoleDockController<TTarget> {
  const panelCounterRef = useRef(1);
  const splitCounterRef = useRef(1);
  const tabCounterRef = useRef(1);

  function nextPanelId() {
    return `panel-${panelCounterRef.current++}`;
  }

  function nextSplitId() {
    return `split-${splitCounterRef.current++}`;
  }

  function nextTabId() {
    return `tab-${tabCounterRef.current++}`;
  }

  const [state, setState] = useState<ConsoleDockState<TTarget>>(() => createConsoleDockState({
    initialTarget,
    initialPresetId,
    createPanelState: (args) => {
      const nextState = createPanelState(args);
      return {
        ...nextState,
        id: nextState.id || nextPanelId(),
      };
    },
    createSplitId: nextSplitId,
    createTabId: nextTabId,
    suggestTargets,
  }));

  const viewState = useMemo(() => buildConsoleDockViewState<TTarget>(state, {
    resolvePanelView,
    resolveTabView,
  }), [resolvePanelView, resolveTabView, state]);

  const focusedPanel = useMemo(
    () => state.panels.find((panel) => panel.id === state.focusedPanelId) || null,
    [state.focusedPanelId, state.panels],
  );

  function dispatch(action: ConsoleDockAction<TTarget>) {
    setState((current) => applyConsoleDockAction<TTarget>(current, action, {
      createPanelState: (args) => {
        const nextState = createPanelState(args);
        return {
          ...nextState,
          id: nextState.id || nextPanelId(),
        };
      },
      createSplitId: nextSplitId,
      createTabId: nextTabId,
      suggestTargets,
    }));
  }

  return {
    state,
    setState,
    viewState,
    presets: consoleDockPresets(),
    focusedPanel,
    focusedPanelId: state.focusedPanelId,
    focusedTarget: focusedPanel?.target || null,
    dispatch,
    createTab: (options) => dispatch({ type: "create_tab", blank: options?.blank }),
    selectTab: (tabId) => dispatch({ type: "select_tab", tabId }),
    closeTab: (tabId) => dispatch({ type: "close_tab", tabId }),
    focusPanel: (panelId) => dispatch({ type: "focus_panel", panelId }),
    closePanel: (panelId) => dispatch({ type: "close_panel", panelId }),
    splitPanel: (panelId, direction) => dispatch({ type: "split_panel", panelId, direction }),
    resizeSplit: (splitId, ratio) => dispatch({ type: "resize_split", splitId, ratio }),
    applyPreset: (presetId) => dispatch({ type: "apply_preset", presetId }),
    openTarget: (target, intent) => dispatch({ type: "open_target", target, intent }),
    setPanelTarget: (panelId, target) => dispatch({ type: "set_panel_target", panelId, target }),
    setPanelMode: (panelId, mode) => dispatch({ type: "set_panel_mode", panelId, mode }),
  };
}
