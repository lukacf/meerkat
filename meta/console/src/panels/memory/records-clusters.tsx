// The Records Clusters view: one tile per kind with its count, how its
// records spread over the five trust steps, and the three most used
// titles. A tile narrows the List to its kind.

import React from "react";

import type { MemoryPanelRecord, MemoryRecordKind } from "../../types";
import { StatusMark, cx, plural } from "./kit";
import { KindMark } from "./parts";
import { kindClusters, type KindCluster } from "./viz-model";
import { kindLabel, kindPlural } from "./words";

const TRUST_WORDS = ["untrusted", "observed", "verified", "application", "operator"];


/// The five trust steps as a ladder of bars: height by count, ink deepening
/// with trust.
function TrustSpread({ cluster }: { cluster: KindCluster }): React.JSX.Element {
  const max = Math.max(1, ...cluster.trust);
  const words = cluster.trust
    .map((count, index) => (count > 0 ? `${count} ${TRUST_WORDS[index]}` : ""))
    .filter(Boolean)
    .join(", ");
  return (
    <span className="memory-cluster__spread" role="img" aria-label={words ? `Trust: ${words}` : "No records"}>
      {cluster.trust.map((count, index) => (
        <span key={index} className="memory-cluster__step" data-step={index + 1}>
          <span className="memory-cluster__bar" style={{ height: `${count > 0 ? Math.max(3, (count / max) * 22) : 1}px` }} data-zero={count === 0 ? "true" : undefined} />
        </span>
      ))}
    </span>
  );
}

export function RecordsClusters({
  records,
  selectedKinds,
  onPick,
  onOpenRecord,
}: {
  records: ReadonlyArray<MemoryPanelRecord>;
  selectedKinds: ReadonlyArray<MemoryRecordKind>;
  onPick: (kind: MemoryRecordKind) => void;
  onOpenRecord: (realm: string | undefined, memoryId: string) => void;
}): React.JSX.Element {
  const clusters = React.useMemo(() => kindClusters(records), [records]);
  const total = Math.max(1, records.length);
  return (
    <div className="memory-clusters" data-testid="memory-records-clusters">
      {clusters.map((cluster) => (
        <section
          key={cluster.kind}
          className={cx("memory-cluster", cluster.count === 0 && "is-empty", selectedKinds.includes(cluster.kind) && "is-selected")}
          data-testid={`memory-cluster:${cluster.kind}`}
        >
          <button
            type="button"
            className="memory-cluster__hit"
            disabled={cluster.count === 0}
            onClick={() => onPick(cluster.kind)}
            aria-label={`Show the ${plural(cluster.count, kindLabel(cluster.kind).toLowerCase())} in the list`}
          >
            <KindMark kind={cluster.kind} className="memory-cluster__mark" />
            <span className="memory-cluster__name">{kindPlural(cluster.kind)}</span>
            <span className="memory-cluster__count">{cluster.count}</span>
          </button>
          <span className="memory-cluster__share" aria-hidden="true">
            <span className="memory-cluster__share-fill" style={{ width: `${(cluster.count / total) * 100}%` }} />
          </span>
          <div className="memory-cluster__trust">
            <TrustSpread cluster={cluster} />
            <span className="memory-cluster__trust-label">
              {cluster.count === 0 ? "None yet" : `${Math.round((cluster.count / total) * 100)}% of loaded records`}
            </span>
          </div>
          {cluster.top.length > 0 ? (
            <ol className="memory-cluster__top" aria-label="Most used">
              {cluster.top.map((entry) => (
                <li key={entry.id}>
                  <button type="button" className="memory-link memory-cluster__record" onClick={() => onOpenRecord(entry.realm, entry.id)}>
                    {entry.title}
                  </button>
                  {entry.uses > 0 ? <span className="memory-cluster__uses">{`used ${entry.uses}`}</span> : null}
                </li>
              ))}
            </ol>
          ) : null}
          {cluster.quarantined > 0 ? (
            <span className="memory-cluster__flag">
              <StatusMark tone="warn" />
              {`${cluster.quarantined} quarantined`}
            </span>
          ) : null}
        </section>
      ))}
    </div>
  );
}
