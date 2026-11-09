// EchoText — free-text inputs over server-authoritative state.
//
// Every keystroke in the editor round-trips meta/mobpacks/apply_operation
// and the document echo re-renders the input. A plainly controlled
// input/textarea breaks under that: React restores the DOM to the (stale)
// value prop after each onChange, snapping the caret to the end, so typing
// mid-string lands characters at the end of the field.
//
// EchoText keeps a local draft while the field is focused — the controlled
// value always matches the DOM, so the caret never moves. While unfocused
// the server document is the only truth: any value change (echo, undo/redo,
// selection switch) syncs the draft. Key usages by the edited entity's id so
// switching selection remounts with clean state.
//
// On blur the draft is force-resynced to the server-authoritative `value`,
// not just on `value` change. A per-keystroke apply_operation that FAILS
// (RPC error / injected backend failure / runner stale-drop) leaves the
// document value unchanged, so the `[value]`-dep effect never fires and the
// typed character would otherwise stay in the field forever — displaying text
// that is not in the deployable document. Resyncing on blur drops that
// phantom; any in-flight op that lands later still resyncs through the
// unfocused `[value]` effect.

export function useEchoDraft(value) {
  const [draft, setDraft] = React.useState(value ?? "");
  const focusedRef = React.useRef(false);
  React.useEffect(() => {
    if (!focusedRef.current) setDraft(value ?? "");
  }, [value]);
  return { draft, setDraft, focusedRef };
}

function echoProps({ value, onChangeText, onFocus, onBlur, ...rest }, draftState) {
  const { draft, setDraft, focusedRef } = draftState;
  return {
    ...rest,
    value: draft,
    onFocus: (e) => {
      focusedRef.current = true;
      if (onFocus) onFocus(e);
    },
    onBlur: (e) => {
      focusedRef.current = false;
      // Server-authoritative resync: snap back to the committed document
      // value so a failed per-keystroke op cannot strand phantom text in
      // the field after the user leaves it.
      setDraft(value ?? "");
      if (onBlur) onBlur(e);
    },
    onChange: (e) => {
      setDraft(e.target.value);
      if (onChangeText) onChangeText(e.target.value);
    },
  };
}

export function EchoInput(props) {
  const draftState = useEchoDraft(props.value);
  return <input {...echoProps(props, draftState)} />;
}

export function EchoTextArea(props) {
  const draftState = useEchoDraft(props.value);
  return <textarea {...echoProps(props, draftState)} />;
}
