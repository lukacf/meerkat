import React from "react";

/** Keep current progress visible while retaining every supplied history entry. */
export function DeploymentActivity({ receipt, active = false }) {
  const progress = Array.isArray(receipt?.progress) ? receipt.progress : [];
  if (!progress.length) return null;
  const split = Math.max(0, progress.length - 5);
  const earlier = progress.slice(0, split), recent = progress.slice(split).reverse();
  return <details key={`${receipt.guide_id}:${receipt.deployment_id}`} className="deployment-evidence"
    open={active && receipt.status !== "running"}>
    <summary>Deployment activity</summary>
    {progress.length > 1 && <p>{earlier.length > 0 ? `Latest ${recent.length} of ${progress.length} saved updates. ` : ""}Newest first.</p>}
    <ol reversed start={progress.length} aria-label="Latest deployment updates, newest first">
      {recent.map((entry, index) => <li key={progress.length - index}>{entry.message}</li>)}
    </ol>
    {earlier.length > 0 && <details>
      <summary>Earlier activity ({earlier.length} {earlier.length === 1 ? "update" : "updates"})</summary>
      <ol aria-label="Earlier deployment activity">
        {earlier.map((entry, index) => <li key={index}>{entry.message}</li>)}
      </ol>
    </details>}
  </details>;
}
