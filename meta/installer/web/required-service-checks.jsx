import React from "react";

// Presentation only. Every requirement and its verification state is retained.
export function RequiredServiceChecks({ operations }) {
  if (!operations.length) return null;
  const groups = new Map();
  for (const item of operations) {
    const key = JSON.stringify([item.service, item.resource || null]);
    if (!groups.has(key)) groups.set(key, { service: item.service, resource: item.resource, items: [] });
    groups.get(key).items.push(item);
  }
  return <details className="implementation-service-groups">
    <summary>Required operations and resources ({operations.length})</summary>
    {[...groups].map(([key, group]) =>
    <section key={key}>
      <strong>{group.service}</strong><p>{group.resource || "Choose the resource this service should use."}</p>
        <ul>{group.items.map((item, index) => <li key={`${item.id}:${index}`}>{item.operation}
          {item.capability === "incoming_interaction" && <span> - receiving messages needs its own check</span>}
          {item.capability === "unresolved" && (item.reason_code === "source_binding_not_verified" && item.planned_operations?.length
            ? <><p>{item.planned_operations.length} planned operations; source setup still needs checking</p><p>The connection and access to each resource still need verification.</p><RequiredServiceChecks operations={item.planned_operations} /></>
            : <span> - required operations still need selection</span>)}
        </li>)}</ul>
    </section>)}</details>;
}
