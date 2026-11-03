// These predicates borrow the wizard's existing load lifetime. They do not
// own guide selection, operation admission, or domain receipt currency.
export function captureGuideSelection(scope, guide, expectedToken) {
  const current = scope ? scope.capture(guide, expectedToken) : () => true;
  return typeof current === "function" && current() ? current : null;
}

export function guideSelectionAvailable(scope, guide, expectedToken) {
  return !!captureGuideSelection(scope, guide, expectedToken);
}

export function guideSelectionEvents(scope, guide, expectedToken) {
  const guard = (event) => {
    if (guideSelectionAvailable(scope, guide, expectedToken)) return;
    event.preventDefault();
    event.stopPropagation();
  };
  return { onClickCapture: guard, onSubmitCapture: guard, onChangeCapture: guard };
}
