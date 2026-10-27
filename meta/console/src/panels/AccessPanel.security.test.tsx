import React from "react";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AccessPanel, type AccessPreviewResult } from "./AccessPanel";

function props() {
  return {
    status: { available: true, enabled: true, revision: 1, owner_instance: "panel-owner", conditional_mutations: "checked_v1", subject: "admin@example.test", can_administer: true, actions: ["agent.view", "agent.send"] },
    config: { enabled: true, admins: ["admin@example.test"], rules: [], groups: {} },
    agents: [{ identity: "identity:agent", label: "Agent" }],
    onRefresh: vi.fn(), onSetEnabled: vi.fn(), onSaveAdmins: vi.fn(), onUpsertRule: vi.fn(),
    onDeleteRule: vi.fn(), onSaveGroup: vi.fn(), onDeleteGroup: vi.fn(),
    onPreview: vi.fn(async (): Promise<AccessPreviewResult> => ({ allowed: true })),
  };
}

describe("backend-owned access affordances", () => {
  it("does not infer administration permission from a loaded configuration", () => {
    const input = props();
    render(<AccessPanel {...input} status={{ ...input.status, can_administer: false }} />);
    expect(screen.queryByTestId("access-toggle-enabled")).toBeNull();
    expect(screen.queryByTestId("access-edit-admins")).toBeNull();
    expect(screen.queryByTestId("access-rule-new")).toBeNull();
    expect(screen.queryByText("admin@example.test")).toBeNull();
    expect(input.onSetEnabled).not.toHaveBeenCalled();
    expect(input.onUpsertRule).not.toHaveBeenCalled();
  });

  it("does not invent an action vocabulary when the owner supplied none", () => {
    const input = props();
    render(<AccessPanel {...input} status={{ ...input.status, actions: [] }} />);
    fireEvent.click(screen.getByTestId("access-tab:preview"));
    expect(screen.queryByRole("option", { name: "agent.view", exact: true })).toBeNull();
    fireEvent.change(screen.getByTestId("access-preview-subject"), { target: { value: "operator@example.test" } });
    expect(screen.getByTestId("access-preview-run")).toBeDisabled();
    expect(input.onPreview).not.toHaveBeenCalled();
  });

  it("invalidates a preview when its input changes, including an in-flight old answer", async () => {
    const input = props();
    let resolve!: (value: AccessPreviewResult) => void;
    input.onPreview = vi.fn(() => new Promise(done => { resolve = done; }));
    render(<AccessPanel {...input} />);
    fireEvent.click(screen.getByTestId("access-tab:preview"));
    const subject = screen.getByTestId("access-preview-subject");
    fireEvent.change(subject, { target: { value: "first@example.test" } });
    fireEvent.click(screen.getByTestId("access-preview-run"));
    expect(input.onPreview).toHaveBeenCalledWith("first@example.test", "agent.view", undefined);
    fireEvent.change(subject, { target: { value: "second@example.test" } });
    await act(async () => { resolve({ allowed: true, reason: "old answer" }); });
    expect(screen.queryByTestId("access-preview-result")).toBeNull();
  });

  it("retains only the server's current explicit action list", () => {
    const input = props();
    render(<AccessPanel {...input} status={{ ...input.status, actions: ["custom.current.action"] }} />);
    fireEvent.click(screen.getByTestId("access-tab:preview"));
    expect(screen.getByRole("option", { name: "custom.current.action" })).toBeVisible();
    expect(screen.queryByRole("option", { name: "access.admin", exact: true })).toBeNull();
    expect(screen.getByTestId("access-preview-action")).toHaveValue("custom.current.action");
  });
  it("invalidates completed and pending previews on owner revision refresh", async () => {
    const input = props();
    const view = render(<AccessPanel {...input} />);
    fireEvent.click(screen.getByTestId("access-tab:preview"));
    fireEvent.change(screen.getByTestId("access-preview-subject"), { target: { value: "reader" } });
    await act(async () => { fireEvent.click(screen.getByTestId("access-preview-run")); });
    expect(screen.getByTestId("access-preview-result")).toBeVisible();
    view.rerender(<AccessPanel {...input} status={{ ...input.status, revision: 2 }} />);
    expect(screen.queryByTestId("access-preview-result")).toBeNull();
  });

  it("preserves a failed rule draft and gates writes while owner state is stale", async () => {
    const input = props();
    input.onUpsertRule = vi.fn().mockRejectedValue(new Error("Save failed"));
    const view = render(<AccessPanel {...input} />);
    fireEvent.click(screen.getByTestId("access-tab:rules"));
    fireEvent.click(screen.getByTestId("access-rule-new"));
    fireEvent.change(screen.getByTestId("access-rule-id"), { target: { value: "keep-my-draft" } });
    await act(async () => { fireEvent.click(screen.getByTestId("access-rule-save")); });
    expect(screen.getByTestId("access-rule-id")).toHaveValue("keep-my-draft");
    view.rerender(<AccessPanel {...input} error="Owner refresh unavailable" />);
    expect(screen.getByTestId("access-rule-id")).toHaveValue("keep-my-draft");
    expect(screen.getByTestId("access-rule-save")).toBeDisabled();
    fireEvent.click(screen.getByTestId("access-rule-save"));
    expect(input.onUpsertRule).toHaveBeenCalledTimes(1);
  });

  it("does not expose loaded protected data when unavailable or authority is lost", () => {
    const input = props();
    const view = render(<AccessPanel {...input} />);
    expect(screen.getByText("admin@example.test")).toBeVisible();
    view.rerender(<AccessPanel {...input} status={{ ...input.status, available: false }} />);
    expect(screen.queryByText("admin@example.test")).toBeNull();
    expect(screen.queryByTestId("access-toggle-enabled")).toBeNull();
  });

  it("keeps read-only administration disabled without hiding authorized configuration", () => {
    const input = props();
    render(<AccessPanel {...input} readOnly />);
    expect(screen.getByText("admin@example.test")).toBeVisible();
    expect(screen.queryByTestId("access-toggle-enabled")).toBeNull();
  });

});

describe("preview decision and error scope", () => {
  it.each(["before", "during"])("preserves a read-only preview completed %s an unchanged refresh", async (completion) => {
    const input = props();
    let resolve!: (value: AccessPreviewResult) => void;
    input.onPreview = vi.fn(() => new Promise(done => { resolve = done; }));
    const view = render(<AccessPanel {...input} readOnly />);
    fireEvent.click(screen.getByTestId("access-tab:preview"));
    fireEvent.change(screen.getByTestId("access-preview-subject"), { target: { value: "reader" } });
    fireEvent.click(screen.getByTestId("access-preview-run"));
    if (completion === "before") {
      await act(async () => { resolve({ allowed: false, reason: "Current denial" }); });
      expect(screen.getByTestId("access-preview-result")).toHaveAttribute("data-allowed", "false");
    }
    view.rerender(<AccessPanel {...input} readOnly loading />);
    expect(screen.queryByTestId("access-preview-result")).toBeNull();
    expect(screen.getByTestId("access-preview-run")).toBeDisabled();
    if (completion === "during") {
      await act(async () => { resolve({ allowed: false, reason: "Current denial" }); });
      expect(screen.queryByTestId("access-preview-result")).toBeNull();
    }
    view.rerender(<AccessPanel {...input} readOnly />);
    expect(screen.getByTestId("access-preview-result")).toHaveAttribute("data-allowed", "false");
    expect(input.onPreview).toHaveBeenCalledTimes(1);
    expect(input.onSetEnabled).not.toHaveBeenCalled();
  });

  it.each(["owner", "revision", "config", "error"])("discards a pending preview when refresh changes %s", async (change) => {
    const input = props();
    let resolve!: (value: AccessPreviewResult) => void;
    input.onPreview = vi.fn(() => new Promise(done => { resolve = done; }));
    const view = render(<AccessPanel {...input} />);
    fireEvent.click(screen.getByTestId("access-tab:preview"));
    fireEvent.change(screen.getByTestId("access-preview-subject"), { target: { value: "reader" } });
    fireEvent.click(screen.getByTestId("access-preview-run"));
    view.rerender(<AccessPanel {...input} loading />);
    const refreshed = {
      ...input,
      ...(change === "owner" ? { status: { ...input.status, owner_instance: "replacement-owner" } } : {}),
      ...(change === "revision" ? { status: { ...input.status, revision: 2 } } : {}),
      ...(change === "config" ? { config: { ...input.config, enabled: false } } : {}),
      ...(change === "error" ? { error: "Refresh unavailable" } : {}),
    };
    view.rerender(<AccessPanel {...refreshed} />);
    await act(async () => { resolve({ allowed: true, reason: "Old decision" }); });
    expect(screen.queryByTestId("access-preview-result")).toBeNull();
    view.rerender(<AccessPanel {...input} />);
    expect(screen.queryByTestId("access-preview-result")).toBeNull();
  });

  it.each(["owner", "revision", "config"])("never commits a retained preview under changed %s", async (change) => {
    const input = props();
    let resolve!: (value: AccessPreviewResult) => void;
    input.onPreview = vi.fn(() => new Promise(done => { resolve = done; }));
    let decisionAtCommit: string | null | undefined;
    function CommitProbe({ value }: { value: React.ComponentProps<typeof AccessPanel> }) {
      React.useLayoutEffect(() => {
        decisionAtCommit = document.querySelector('[data-testid="access-preview-result"]')?.textContent ?? null;
      });
      return <AccessPanel {...value} />;
    }
    const view = render(<CommitProbe value={input} />);
    fireEvent.click(screen.getByTestId("access-tab:preview"));
    fireEvent.change(screen.getByTestId("access-preview-subject"), { target: { value: "reader" } });
    fireEvent.click(screen.getByTestId("access-preview-run"));
    view.rerender(<CommitProbe value={{ ...input, loading: true }} />);
    await act(async () => { resolve({ allowed: true, reason: "Old decision" }); });
    const refreshed = {
      ...input,
      ...(change === "owner" ? { status: { ...input.status, owner_instance: "replacement-owner" } } : {}),
      ...(change === "revision" ? { status: { ...input.status, revision: 2 } } : {}),
      ...(change === "config" ? { config: { ...input.config, enabled: false } } : {}),
    };
    view.rerender(<CommitProbe value={refreshed} />);
    expect(decisionAtCommit).toBeNull();
  });

  it.each(["missing", "rejected"])("keeps %s decision unavailable without exposing private payload", async (kind) => {
    const input = props();
    input.onPreview = kind === "missing" ? vi.fn(async () => ({ reason: "PRIVATE_PREVIEW_ERROR" })) : vi.fn().mockRejectedValue(new Error("PRIVATE_PREVIEW_ERROR"));
    render(<AccessPanel {...input} />);
    fireEvent.click(screen.getByTestId("access-tab:preview"));
    fireEvent.change(screen.getByTestId("access-preview-subject"), { target: { value: "reader" } });
    await act(async () => { fireEvent.click(screen.getByTestId("access-preview-run")); });
    expect(screen.queryByTestId("access-preview-result")).toBeNull();
    expect(screen.getByTestId("access-preview-error")).toHaveTextContent("unavailable");
    expect(document.body).not.toHaveTextContent("PRIVATE_PREVIEW_ERROR");
    expect(screen.getByTestId("access-preview-run")).toBeEnabled();
  });
  it("ignores a rejected old preview after the authenticated subject changes", async () => {
    const input = props();
    let reject!: (error: Error) => void;
    input.onPreview = vi.fn(() => new Promise((_resolve, fail) => { reject = fail; }));
    const view = render(<AccessPanel {...input} />);
    fireEvent.click(screen.getByTestId("access-tab:preview"));
    fireEvent.change(screen.getByTestId("access-preview-subject"), { target: { value: "reader" } });
    fireEvent.click(screen.getByTestId("access-preview-run"));
    view.rerender(<AccessPanel {...input} status={{ ...input.status, subject: "next-admin" }} />);
    await act(async () => { reject(new Error("OLD_SCOPE_PRIVATE_ERROR")); });
    expect(screen.queryByTestId("access-preview-error")).toBeNull();
    expect(document.body).not.toHaveTextContent("OLD_SCOPE_PRIVATE_ERROR");
    expect(screen.getByTestId("access-preview-run")).toBeEnabled();
  });
});

describe("group member access inspection", () => {
  const subject = "service:operations/shift-7";

  it("prefills an exact subject and evaluates only when explicitly requested", async () => {
    const input = props();
    const config = { ...input.config, groups: { operations: { members: [subject] } } };
    render(<AccessPanel {...input} config={config} />);
    fireEvent.click(screen.getByTestId("access-tab:groups"));
    fireEvent.click(screen.getByRole("button", { name: `Inspect access for ${subject}` }));
    expect(screen.getByTestId("access-preview-subject")).toHaveValue(subject);
    expect(screen.getByTestId("access-preview-subject")).toHaveFocus();
    expect(screen.queryByTestId("access-preview-result")).toBeNull();
    expect(input.onPreview).not.toHaveBeenCalled();
    await act(async () => { fireEvent.click(screen.getByTestId("access-preview-run")); });
    expect(input.onPreview).toHaveBeenCalledExactlyOnceWith(subject, "agent.view", undefined);
    expect(screen.getByTestId("access-preview-result")).toBeVisible();

    fireEvent.click(screen.getByTestId("access-tab:groups"));
    fireEvent.click(screen.getByRole("button", { name: `Inspect access for ${subject}` }));
    expect(screen.getByTestId("access-preview-subject")).toHaveValue(subject);
    expect(screen.queryByTestId("access-preview-result")).toBeNull();
    expect(input.onPreview).toHaveBeenCalledTimes(1);
    expect(input.onSaveGroup).not.toHaveBeenCalled();
  });

  it("ignores an old in-flight answer after selecting a group member", async () => {
    const input = props();
    let resolve!: (value: AccessPreviewResult) => void;
    input.onPreview = vi.fn(() => new Promise(done => { resolve = done; }));
    const config = { ...input.config, groups: { operations: { members: [subject] } } };
    render(<AccessPanel {...input} config={config} />);
    fireEvent.click(screen.getByTestId("access-tab:preview"));
    fireEvent.change(screen.getByTestId("access-preview-subject"), { target: { value: "old-subject" } });
    fireEvent.click(screen.getByTestId("access-preview-run"));
    expect(input.onPreview).toHaveBeenCalledWith("old-subject", "agent.view", undefined);
    fireEvent.click(screen.getByTestId("access-tab:groups"));
    fireEvent.click(screen.getByRole("button", { name: `Inspect access for ${subject}` }));
    expect(screen.getByTestId("access-preview-subject")).toHaveValue(subject);
    expect(screen.getByTestId("access-preview-run")).toBeEnabled();
    await act(async () => { resolve({ allowed: true, reason: "OLD_SUBJECT_ANSWER" }); });
    expect(screen.queryByTestId("access-preview-result")).toBeNull();
    expect(document.body).not.toHaveTextContent("OLD_SUBJECT_ANSWER");
    expect(input.onPreview).toHaveBeenCalledTimes(1);
  });

  it("allows read-only inspection without exposing policy mutations", async () => {
    const input = props();
    const config = { ...input.config, groups: { operations: { members: [subject] } } };
    render(<AccessPanel {...input} config={config} readOnly />);
    fireEvent.click(screen.getByTestId("access-tab:groups"));
    expect(screen.queryByTestId("access-group-edit:operations")).toBeNull();
    expect(screen.queryByTestId("access-group-delete:operations")).toBeNull();
    expect(screen.queryByTestId("access-group-save")).toBeNull();
    const inspect = screen.getByRole("button", { name: `Inspect access for ${subject}` });
    expect(inspect).toBeEnabled();
    fireEvent.click(inspect);
    expect(input.onPreview).not.toHaveBeenCalled();
    await act(async () => { fireEvent.click(screen.getByTestId("access-preview-run")); });
    expect(input.onPreview).toHaveBeenCalledExactlyOnceWith(subject, "agent.view", undefined);
    expect(input.onSaveGroup).not.toHaveBeenCalled();
    expect(input.onDeleteGroup).not.toHaveBeenCalled();
    expect(input.onSetEnabled).not.toHaveBeenCalled();
  });

  it.each(["unavailable", "not-admin", "refreshing", "stale"])("keeps inspection unavailable when %s", (kind) => {
    const input = props();
    const config = { ...input.config, groups: { operations: { members: [subject] } } };
    const status = { ...input.status, available: kind !== "unavailable", can_administer: kind !== "not-admin" };
    render(<AccessPanel {...input} config={config} status={status}
      loading={kind === "refreshing"} error={kind === "stale" ? "Owner unavailable" : null} />);
    const groups = screen.queryByTestId("access-tab:groups");
    if (groups) fireEvent.click(groups);
    const inspect = screen.queryByRole("button", { name: `Inspect access for ${subject}` });
    if (inspect) {
      expect(inspect).toBeDisabled();
      fireEvent.click(inspect);
    }
    expect(screen.queryByTestId("access-preview-subject")).toBeNull();
    expect(input.onPreview).not.toHaveBeenCalled();
    expect(input.onSaveGroup).not.toHaveBeenCalled();
  });

  it("does not switch the preview subject while a policy save is pending", async () => {
    const input = props();
    let finish!: (value: boolean) => void;
    input.onSetEnabled = vi.fn(() => new Promise(done => { finish = done; }));
    const config = { ...input.config, groups: { operations: { members: [subject] } } };
    render(<AccessPanel {...input} config={config} />);
    fireEvent.click(screen.getByTestId("access-toggle-enabled"));
    fireEvent.click(screen.getByTestId("access-tab:groups"));
    const inspect = screen.queryByRole("button", { name: `Inspect access for ${subject}` });
    if (inspect) {
      expect(inspect).toBeDisabled();
      fireEvent.click(inspect);
    }
    expect(screen.queryByTestId("access-preview-subject")).toBeNull();
    expect(input.onPreview).not.toHaveBeenCalled();
    await act(async () => { finish(true); });
    expect(screen.getByRole("button", { name: `Inspect access for ${subject}` })).toBeEnabled();
    expect(input.onSaveGroup).not.toHaveBeenCalled();
  });
});

describe("checked access edits", () => {
  it.each([
    ["no checked capability", { conditional_mutations: undefined }],
    ["an unknown checked capability", { conditional_mutations: "checked_v2" }],
    ["an empty owner instance", { owner_instance: "" }],
    ["no revision", { revision: undefined }],
    ["a negative revision", { revision: -1 }],
  ])("keeps authorized reads and inspection but offers no edit with %s", async (_label, change) => {
    const input = props();
    const config = { ...input.config, groups: { ops: { members: ["reader@example.test"] } } };
    render(<AccessPanel {...input} config={config} status={{ ...input.status, ...change }} />);
    expect(screen.getByText("admin@example.test")).toBeVisible();
    expect(screen.queryByTestId("access-toggle-enabled")).toBeNull();
    expect(screen.queryByTestId("access-edit-admins")).toBeNull();
    fireEvent.click(screen.getByTestId("access-tab:rules"));
    expect(screen.queryByTestId("access-rule-new")).toBeNull();
    fireEvent.click(screen.getByTestId("access-tab:groups"));
    expect(screen.queryByTestId("access-group-edit:ops")).toBeNull();
    expect(screen.queryByTestId("access-group-save")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Inspect access for reader@example.test" }));
    await act(async () => { fireEvent.click(screen.getByTestId("access-preview-run")); });
    expect(input.onPreview).toHaveBeenCalledExactlyOnceWith("reader@example.test", "agent.view", undefined);
    for (const write of [input.onSetEnabled, input.onSaveAdmins, input.onUpsertRule, input.onDeleteRule, input.onSaveGroup, input.onDeleteGroup]) {
      expect(write).not.toHaveBeenCalled();
    }
  });

  it.each([
    ["revision_conflict", "Access configuration changed. Review the latest settings before saving again.", true],
    ["owner_changed", "The access configuration owner changed. Review the latest settings before saving again.", true],
    ["unavailable", "Changes were not saved. Checked access saves are unavailable; your draft is retained.", true],
    ["invalid", "Changes were not saved. The resulting access configuration is not valid; your draft is retained for correction.", false],
    ["failed", "Changes were not saved. Your draft is retained; refresh Console access before trying again.", false],
  ] as const)("keeps the admins draft after %s with its finite notice and review requirement", async (kind, notice, review) => {
    const input = props();
    input.onSaveAdmins = vi.fn(async () => ({ kind }));
    render(<AccessPanel {...input} />);
    fireEvent.click(screen.getByTestId("access-edit-admins"));
    fireEvent.change(screen.getByTestId("access-admins-input"), { target: { value: "admin@example.test, new@example.test" } });
    await act(async () => { fireEvent.click(screen.getByTestId("access-save-admins")); });
    expect(input.onSaveAdmins).toHaveBeenCalledExactlyOnceWith(["admin@example.test", "new@example.test"],
      { owner_instance: "panel-owner", revision: 1, config: input.config });
    expect(screen.getByTestId("access-error").textContent).toBe(notice);
    expect(screen.getByTestId("access-admins-input")).toHaveValue("admin@example.test, new@example.test");
    const reviewButton = screen.queryByRole("button", { name: "Review and reapply", exact: true });
    if (review) {
      expect(reviewButton).toBeVisible();
      expect(screen.getByTestId("access-save-admins")).toBeDisabled();
    } else {
      expect(reviewButton).toBeNull();
      expect(screen.getByTestId("access-save-admins")).toBeEnabled();
    }
  });

  // Edits belong to the scope they were opened under, and the first commit
  // under a changed scope already shows none of them. The reset used to run
  // as an effect after that commit, so an editor opened against the commit
  // (the integration suite's checked saves, right as the status loaded) was
  // closed again by the reset still pending from it.
  it("discards edits in the commit that shows a changed scope and keeps one opened under it", () => {
    const input = props();
    const committed: boolean[] = [];
    // A layout effect reads the DOM of its own commit, before that commit's
    // passive effects run.
    function CommitProbe() {
      React.useLayoutEffect(() => { committed.push(screen.queryByTestId("access-admins-input") !== null); });
      return null;
    }
    const view = render(<><AccessPanel {...input} /><CommitProbe /></>);
    fireEvent.click(screen.getByTestId("access-edit-admins"));
    expect(screen.getByTestId("access-admins-input")).toBeVisible();
    committed.length = 0;
    view.rerender(<><AccessPanel {...input} status={{ ...input.status, subject: "next-admin@example.test" }} /><CommitProbe /></>);
    expect(committed).toEqual([false]);
    expect(screen.queryByTestId("access-admins-input")).toBeNull();
    fireEvent.click(screen.getByTestId("access-edit-admins"));
    expect(screen.getByTestId("access-admins-input")).toBeVisible();
  });

  it("keeps an invalid rule draft editable against its own base without a review step", async () => {
    const input = props();
    input.onUpsertRule = vi.fn().mockResolvedValueOnce({ kind: "invalid" }).mockResolvedValueOnce(true);
    render(<AccessPanel {...input} />);
    fireEvent.click(screen.getByTestId("access-tab:rules"));
    fireEvent.click(screen.getByTestId("access-rule-new"));
    fireEvent.change(screen.getByTestId("access-rule-id"), { target: { value: "ops-rule" } });
    fireEvent.change(screen.getByTestId("access-rule-groups"), { target: { value: "missing-group" } });
    await act(async () => { fireEvent.click(screen.getByTestId("access-rule-save")); });
    expect(screen.getByTestId("access-error")).toHaveTextContent("The resulting access configuration is not valid");
    expect(screen.queryByRole("button", { name: "Review and reapply", exact: true })).toBeNull();
    expect(screen.getByTestId("access-rule-groups")).toHaveValue("missing-group");
    fireEvent.change(screen.getByTestId("access-rule-groups"), { target: { value: "" } });
    await act(async () => { fireEvent.click(screen.getByTestId("access-rule-save")); });
    const base = { owner_instance: "panel-owner", revision: 1, config: input.config };
    expect(input.onUpsertRule).toHaveBeenNthCalledWith(1, { id: "ops-rule", effect: "allow", actions: ["agent.view"], groups: ["missing-group"] }, base);
    expect(input.onUpsertRule).toHaveBeenNthCalledWith(2, { id: "ops-rule", effect: "allow", actions: ["agent.view"] }, base);
    expect(screen.queryByTestId("access-rule-editor")).toBeNull();
    expect(screen.queryByTestId("access-error")).toBeNull();
  });

  it("disables a conflicted group draft beside live member inspection until an explicit review", async () => {
    const input = props();
    const member = "reader@example.test";
    const config = { ...input.config, groups: {
      ops: { description: "Original description", members: ["alice@example.test"] },
      readers: { members: [member] },
    } };
    input.onSaveGroup = vi.fn().mockResolvedValueOnce({ kind: "revision_conflict" }).mockResolvedValueOnce(true);
    const view = render(<AccessPanel {...input} config={config} />);
    fireEvent.click(screen.getByTestId("access-tab:groups"));
    fireEvent.click(screen.getByTestId("access-group-edit:ops"));
    fireEvent.change(screen.getByTestId("access-group-members"), { target: { value: "carol@example.test" } });
    await act(async () => { fireEvent.click(screen.getByTestId("access-group-save")); });
    expect(input.onSaveGroup).toHaveBeenLastCalledWith("ops", { description: "Original description", members: ["carol@example.test"] },
      { owner_instance: "panel-owner", revision: 1, config });

    const newer = { ...config, groups: { ...config.groups, ops: { description: "Newer description", members: ["alice@example.test"] } } };
    view.rerender(<AccessPanel {...input} config={newer} status={{ ...input.status, revision: 2 }} />);
    expect(screen.getByTestId("access-error")).toHaveTextContent("Access configuration changed.");
    expect(screen.getByTestId("access-group-members")).toHaveValue("carol@example.test");
    expect(screen.getByTestId("access-group-members")).toBeDisabled();
    expect(screen.getByTestId("access-group-save")).toBeDisabled();
    expect(screen.getByRole("button", { name: "Cancel", exact: true })).toBeDisabled();
    expect(screen.getByRole("button", { name: `Inspect access for ${member}` })).toBeEnabled();
    fireEvent.click(screen.getByTestId("access-group-save"));
    expect(input.onSaveGroup).toHaveBeenCalledTimes(1);

    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Review and reapply", exact: true })); });
    expect(input.onSaveGroup).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId("access-group-members")).toHaveValue("carol@example.test");
    expect(screen.getByTestId("access-group-save")).toBeEnabled();
    await act(async () => { fireEvent.click(screen.getByTestId("access-group-save")); });
    expect(input.onSaveGroup).toHaveBeenLastCalledWith("ops", { description: "Newer description", members: ["carol@example.test"] },
      { owner_instance: "panel-owner", revision: 2, config: newer });
    expect(screen.queryByTestId("access-group-members")).toHaveValue("");
  });
});
