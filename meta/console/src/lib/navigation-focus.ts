// Focus requests (spec A4.7): the console asks the view that shows one kind
// of entity (WorkGraph's items, Routing's routes) to focus on one. ConsoleApp
// raises them through its navigation; a view reads them with
// useNavigationFocus. React only, so any panel's test bundle can import it.

import React from "react";

/** The entities a view focuses on request (WorkGraph's items, Routing's routes). */
export type NavigationFocusKind = "work-item" | "route";

/** A request to show one entity. Each request has its own serial, so asking twice focuses twice. */
export interface NavigationFocusRequest {
  kind: NavigationFocusKind;
  id: string;
  serial: number;
}

interface NavigationFocusValue {
  request: NavigationFocusRequest | null;
  /** The view showed the entity: the request is spent. */
  done(serial: number): void;
}

const NavigationFocusContext = React.createContext<NavigationFocusValue | null>(null);

/** The console's focus requests: `focus` asks, `value` goes to NavigationFocusProvider. */
export function useNavigationFocusSource(): { value: NavigationFocusValue; focus(kind: NavigationFocusKind, id: string): void } {
  const [request, setRequest] = React.useState<NavigationFocusRequest | null>(null);
  const serial = React.useRef(0);
  const focus = React.useCallback((kind: NavigationFocusKind, id: string) => {
    serial.current += 1;
    setRequest({ kind, id, serial: serial.current });
  }, []);
  const done = React.useCallback((handled: number) => {
    setRequest((current) => (current && current.serial === handled ? null : current));
  }, []);
  const value = React.useMemo(() => ({ request, done }), [request, done]);
  return { value, focus };
}

export function NavigationFocusProvider({ value, children }: { value: NavigationFocusValue; children?: React.ReactNode }): React.JSX.Element {
  return React.createElement(NavigationFocusContext.Provider, { value }, children);
}

/**
 * For a view that shows entities of one kind: `onFocus` gets the entity's id
 * once per request (when the view is open, or as it opens). Nothing happens
 * without a provider.
 */
export function useNavigationFocus(kind: NavigationFocusKind, onFocus: (id: string) => void): void {
  const context = React.useContext(NavigationFocusContext);
  const onFocusRef = React.useRef(onFocus);
  onFocusRef.current = onFocus;
  const request = context?.request ?? null;
  const done = context?.done;
  React.useEffect(() => {
    if (!request || request.kind !== kind || !done) return;
    onFocusRef.current(request.id);
    done(request.serial);
  }, [request, kind, done]);
}
