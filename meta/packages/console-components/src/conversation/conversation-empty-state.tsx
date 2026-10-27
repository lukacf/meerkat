import clsx from "clsx";

import type { ConversationEmptyStateSpec } from "@console-core";

import type { IconRenderer } from "../shared";

export type ConversationEmptyStateProps = {
  state: ConversationEmptyStateSpec;
  Icon?: IconRenderer | null;
  className?: string;
  onApplySuggestion?: (value: string) => void;
};

export function ConversationEmptyState({
  state,
  Icon,
  className,
  onApplySuggestion,
}: ConversationEmptyStateProps) {
  return (
    <section className={clsx("cc-empty-state", className)}>
      <div className="cc-empty-state__mark" aria-hidden="true">
        {Icon && state.iconName ? <Icon name={state.iconName} /> : null}
      </div>
      <h2 className="cc-empty-state__title">{state.title}</h2>
      {state.projectLabel ? <div className="cc-empty-state__project">{state.projectLabel}</div> : null}
      <p className="cc-empty-state__subtitle">{state.subtitle}</p>
      {state.suggestions?.length ? (
        <div className="cc-empty-state__actions">
          {state.suggestions.map((suggestion) => (
            <button
              key={suggestion.id}
              type="button"
              className="cc-empty-state__card"
              onClick={() => onApplySuggestion?.(suggestion.value)}
            >
              <span className="cc-empty-state__card-icon" aria-hidden="true">
                {Icon && suggestion.iconName ? <Icon name={suggestion.iconName} /> : null}
              </span>
              <span className="cc-empty-state__card-text">{suggestion.label}</span>
            </button>
          ))}
        </div>
      ) : null}
    </section>
  );
}
