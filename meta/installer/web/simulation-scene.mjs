const W = 1000;
const H = 560;
const PHASE_SECONDS = 4.2;
const DENSE_CLASS_SIZE = 5;
const MEMORY_KINDS = new Set(["notes", "knowledge"]);
const unique = (items) => [...new Set(items)];
const title = (text) => text.replaceAll(/[_-]/g, " ");

function positions(count, start, end) {
  return Array.from({ length: count }, (_, index) =>
    count === 1
      ? (start + end) / 2
      : start + (index * (end - start)) / (count - 1),
  );
}

function ringPositions(count, x, y, inner, outer) {
  const rings = Math.max(
    1,
    Math.min(
      Math.ceil(Math.sqrt(count / 10)),
      Math.floor((outer - inner) / 7) + 1,
    ),
  );
  const radii = positions(rings, inner, outer);
  const total = radii.reduce((sum, radius) => sum + radius, 0);
  let assigned = 0;
  return radii.flatMap((radius, ring) => {
    const amount =
      ring === rings - 1
        ? count - assigned
        : Math.min(
            count - assigned,
            Math.max(1, Math.round((count * radius) / total)),
          );
    assigned += amount;
    return Array.from({ length: amount }, (_, index) => {
      const angle =
        -Math.PI / 2 + ((index + ring * 0.38) * Math.PI * 2) / amount;
      return {
        x: x + Math.cos(angle) * radius,
        y: y + Math.sin(angle) * radius,
        ring,
        radius,
      };
    });
  });
}

function bounds(nodes, padding = 15) {
  if (!nodes.length) return { x: 0, y: 0, width: 0, height: 0 };
  const x = Math.min(...nodes.map((node) => node.x - (node.r || 0))) - padding;
  const y = Math.min(...nodes.map((node) => node.y - (node.r || 0))) - padding;
  return {
    x,
    y,
    width:
      Math.max(...nodes.map((node) => node.x + (node.r || 0))) + padding - x,
    height:
      Math.max(...nodes.map((node) => node.y + (node.r || 0))) + padding - y,
  };
}

function caption(phase) {
  const meaning = {
    access:
      "Highlighted connections show configured access, not completed tool calls.",
    permissions:
      "Links permit collaboration; agents must request and return actual work.",
    handoff: phase.description?.startsWith("An explicit")
      ? "Launch the native workflow with your input."
      : "Direct dependency outputs reach the next steps.",
    output: "Preview of the result path; no real work has run.",
    blocked: "Correct the workflow before it can be previewed.",
  }[phase.kind];
  return meaning ? `${phase.title}. ${meaning}` : phase.title;
}

/** Adapt the actual proposal graph to the original MobSim canvas contract. */
export function createSimulationScene(model, plan) {
  const sourceNodes = model?.nodes || [];
  const sourceEdges = model?.edges || [];
  const sourcePhases = model?.phases || [];
  const agents = sourceNodes.filter((node) => node.kind === "agent");
  const classNames = new Map(
    (plan?.agent_classes || []).map((item) => [item.id, item.name]),
  );
  const classMap = new Map();
  for (const node of agents) {
    if (!classMap.has(node.classId)) classMap.set(node.classId, []);
    classMap.get(node.classId).push(node);
  }
  const firstLayer = new Map();
  sourcePhases
    .filter((phase) => phase.kind === "work")
    .forEach((phase, index) => {
      for (const id of phase.activeNodeIds)
        if (!firstLayer.has(id)) firstLayer.set(id, index);
    });
  const hasFlow = !!model?.selectedFlowId;
  const lastLayer = Math.max(0, ...firstLayer.values());
  const units = [];
  let peerColumn = 0;
  for (const [classId, members] of classMap) {
    const stageMembers = new Map();
    const known = members
      .map((node) => firstLayer.get(node.id))
      .filter((value) => value !== undefined);
    const fallback = hasFlow
      ? (known[0] ?? lastLayer + 1)
      : members.some((node) => node.id === model.entrypoint)
        ? 1
        : (peerColumn++ % 2) * 2;
    for (const node of members) {
      const stage = firstLayer.get(node.id) ?? fallback;
      if (!stageMembers.has(stage)) stageMembers.set(stage, []);
      stageMembers.get(stage).push(node);
    }
    for (const [stage, nodes] of stageMembers) {
      if (members.length >= DENSE_CLASS_SIZE) {
        units.push({
          id: `population:${classId}:${stage}`,
          classId,
          stage,
          nodes,
          dense: true,
        });
      } else {
        for (const node of nodes)
          units.push({
            id: node.id,
            classId,
            stage,
            nodes: [node],
            dense: false,
          });
      }
    }
  }
  const stageValues = unique(units.map((unit) => unit.stage)).sort(
    (a, b) => a - b,
  );
  const columns = Math.min(5, stageValues.length);
  const xPositions = positions(columns, 280, 760);
  const unitColumns = new Map();
  for (const unit of units) {
    const index = stageValues.indexOf(unit.stage);
    const column = Math.min(
      columns - 1,
      Math.floor((index * columns) / stageValues.length),
    );
    unit.column = column;
    if (!unitColumns.has(column)) unitColumns.set(column, []);
    unitColumns.get(column).push(unit);
  }

  const nodes = [];
  const nodeById = new Map();
  const unitForNode = new Map();
  const segments = [];
  const put = (node) => {
    nodes.push(node);
    nodeById.set(node.id, node);
  };
  const labelNodes = [];
  const large = agents.length > 48;
  const denseUnits = units.filter((unit) => unit.dense);
  const actualEntryPeers = new Set(
    sourceEdges
      .filter((edge) => ["peer", "flow"].includes(edge.kind))
      .flatMap((edge) =>
        edge.source === model?.entrypoint
          ? [edge.target]
          : edge.target === model?.entrypoint
            ? [edge.source]
            : [],
      ),
  );
  const aroundEntry = large
    ? denseUnits.filter((unit) =>
        unit.nodes.some((node) => actualEntryPeers.has(node.id)),
      )
    : [];
  const entryRadiusWeight = aroundEntry.reduce(
    (sum, unit) => sum + Math.sqrt(unit.nodes.length),
    0,
  );
  for (const [column, columnUnits] of unitColumns) {
    const band = 310 / columnUnits.length;
    columnUnits.forEach((unit, row) => {
      let x = xPositions[column];
      let y = 115 + band * (row + 0.5);
      if (!hasFlow && !large) {
        const allSpecialists = agents.filter(
          (node) => node.id !== model?.entrypoint,
        );
        if (unit.nodes[0].id === model?.entrypoint) {
          x = 520;
          y = 178;
        } else {
          x =
            positions(allSpecialists.length, 230, 810)[
              allSpecialists.findIndex((node) => node.id === unit.nodes[0].id)
            ] || x;
          y = 306;
        }
      }
      if (large && unit.nodes.some((node) => node.id === model?.entrypoint)) {
        x = 540;
        y = 274;
      }
      const unitNodes = [];
      if (unit.dense) {
        const width = Math.min(
          190,
          columns > 1 ? xPositions[1] - xPositions[0] - 38 : 245,
        );
        const height = Math.max(16, Math.min(180, band - 43));
        const ownerIndex = aroundEntry.indexOf(unit);
        let inner = 12,
          outer = Math.max(18, Math.min(width, height) / 2);
        if (ownerIndex >= 0) {
          x = 540;
          y = 274;
          const preceding = aroundEntry
            .slice(0, ownerIndex)
            .reduce((sum, item) => sum + Math.sqrt(item.nodes.length), 0);
          inner = 48 + (122 * preceding) / entryRadiusWeight;
          outer =
            48 +
            (122 * (preceding + Math.sqrt(unit.nodes.length))) /
              entryRadiusWeight -
            5;
        }
        const points = ringPositions(unit.nodes.length, x, y, inner, outer);
        const radius = unit.nodes.length > 80 ? 1.55 : 2.6;
        unit.nodes.forEach((node, index) => {
          const dot = {
            id: node.id,
            kind: node.id === model?.entrypoint ? "coord" : "agent",
            semanticKind: "agent",
            durable: true,
            dense: true,
            mem: !!node.tools?.memory,
            classId: unit.classId,
            groupId: `class:${unit.classId}`,
            unitId: unit.id,
            x: points[index].x,
            y: points[index].y,
            ring: points[index].ring,
            ringRadius: points[index].radius,
            r: radius,
            label: node.name,
            sub: node.id,
            name: node.name,
            role: node.role,
          };
          if (node.id === model?.entrypoint) {
            dot.x = x;
            dot.y = y;
            dot.r = 5.5;
          }
          put(dot);
          unitNodes.push(dot);
          unitForNode.set(node.id, unit.id);
        });
        labelNodes.push({
          id: `label:${unit.id}`,
          kind: "text",
          x,
          y: ownerIndex >= 0 ? 82 + ownerIndex * 15 : y - outer - 16,
          label: `${classNames.get(unit.classId) || title(unit.classId)} x ${unit.nodes.length}`,
          groupId: `class:${unit.classId}`,
          semanticKind: "population-label",
        });
      } else {
        const node = unit.nodes[0];
        const circle = {
          id: node.id,
          kind: node.id === model?.entrypoint ? "coord" : "agent",
          semanticKind: "agent",
          durable: true,
          mem: !!node.tools?.memory,
          classId: unit.classId,
          groupId: `class:${unit.classId}`,
          unitId: unit.id,
          x,
          y,
          r: node.id === model?.entrypoint ? 5.5 : 4.6,
          label: node.name,
          sub: node.id,
          always: true,
          name: node.name,
          role: node.role,
        };
        put(circle);
        unitNodes.push(circle);
        unitForNode.set(node.id, unit.id);
      }
      const anchor = [...unitNodes].sort(
        (a, b) => Math.hypot(a.x - x, a.y - y) - Math.hypot(b.x - x, b.y - y),
      )[0];
      segments.push({
        id: unit.id,
        classId: unit.classId,
        stage: unit.stage,
        dense: unit.dense,
        ids: unit.nodes.map((node) => node.id),
        count: unit.nodes.length,
        anchorId: anchor.id,
        bounds: bounds(unitNodes, unit.dense ? 14 : 36),
      });
    });
  }
  labelNodes.forEach(put);
  if (!hasFlow && !large && agents.length) {
    [
      { id: "lane:people", label: "PEOPLE", y: 60 },
      { id: "lane:owner", label: "CONVERSATION OWNER", y: 178 },
      ...(agents.length > 1
        ? [{ id: "lane:agents", label: "SPECIALISTS", y: 306 }]
        : []),
      ...(sourceNodes.some(
        (node) =>
          !["agent", "input", "output"].includes(node.kind) &&
          !MEMORY_KINDS.has(node.kind),
      )
        ? [{ id: "lane:tools", label: "CONNECTORS & TOOLS", y: 406 }]
        : []),
      ...(sourceNodes.some((node) => MEMORY_KINDS.has(node.kind))
        ? [{ id: "lane:shared", label: "SHARED", y: 478 }]
        : []),
    ].forEach((node) =>
      put({
        ...node,
        kind: "text",
        semanticKind: "lane-label",
        x: 24,
        align: "left",
      }),
    );
  }

  const external = sourceNodes.filter(
    (node) =>
      !["agent", "input", "output"].includes(node.kind) &&
      !MEMORY_KINDS.has(node.kind),
  );
  const memories = sourceNodes.filter((node) => MEMORY_KINDS.has(node.kind));
  const sourceRows =
    external.length <= 5 ? external.length : Math.ceil(external.length / 2);
  const sourceYs = positions(sourceRows, 110, 410);
  external.forEach((node, index) => {
    put({
      id: node.id,
      kind: "ext",
      semanticKind: node.kind,
      classId: null,
      x:
        !hasFlow && !large
          ? positions(external.length, 220, 820)[index]
          : external.length <= 5
            ? 85
            : 58 + (index % 2) * 112,
      y:
        !hasFlow && !large
          ? 406
          : sourceYs[external.length <= 5 ? index : Math.floor(index / 2)],
      r: 3.6,
      label: node.name,
      sub:
        node.kind === "connector"
          ? "MCP tools"
          : node.role || "Configured access",
      name: node.name,
      role: node.role,
    });
  });
  const memoryXs = positions(memories.length, 350, 670);
  memories.forEach((node, index) =>
    put({
      id: node.id,
      kind: "mem",
      semanticKind: node.kind,
      classId: null,
      x: memoryXs[index],
      y: 478,
      r: 6,
      label: node.name,
      sub: node.kind === "knowledge" ? "Read-only knowledge service" : "Local SQLite",
      name: node.name,
      role: node.role,
    }),
  );
  sourceNodes
    .filter((node) => node.kind === "input")
    .forEach((node) =>
      put({
        id: node.id,
        kind: "ext",
        semanticKind: "input",
        classId: null,
        x: 90,
        y: 59,
        r: 4.2,
        label: node.name,
        sub: "Human interaction",
        labelPos: "above",
        name: node.name,
        role: node.role,
      }),
    );
  sourceNodes
    .filter((node) => node.kind === "output")
    .forEach((node) =>
      put({
        id: node.id,
        kind: "ext",
        semanticKind: "output",
        classId: null,
        x: 930,
        y: 270,
        r: 3.8,
        label: node.name,
        sub: model?.selectedFlowId ? "Native flow output" : "Intended result",
        name: node.name,
        role: node.role,
      }),
    );

  const groups = [...classMap].map(([classId, members]) => {
    const memberNodes = members.map((node) => nodeById.get(node.id));
    const parts = segments.filter((part) => part.classId === classId);
    const groupBounds = bounds(memberNodes, 24);
    return {
      id: `class:${classId}`,
      classId,
      name: classNames.get(classId) || title(classId),
      ids: members.map((node) => node.id),
      memberIds: members.map((node) => node.id),
      ...groupBounds,
      count: members.length,
      renderedCount: members.length,
      representedCount: members.length,
      dense: members.length >= DENSE_CLASS_SIZE,
      anchorId: parts[0].anchorId,
      bounds: groupBounds,
      segments: parts,
      members: members.map((node) => ({ id: node.id, name: node.name })),
    };
  });
  const grouped = agents.length > 48 || sourceEdges.length > 350;
  const edgeMap = new Map();
  const edgeGroupForId = new Map();
  for (const edge of sourceEdges) {
    if (!nodeById.has(edge.source) || !nodeById.has(edge.target)) continue;
    let a = grouped ? unitForNode.get(edge.source) || edge.source : edge.source;
    let b = grouped ? unitForNode.get(edge.target) || edge.target : edge.target;
    if (edge.kind === "peer" && a > b) [a, b] = [b, a];
    const key = grouped ? JSON.stringify([edge.kind, a, b]) : edge.id;
    if (!edgeMap.has(key))
      edgeMap.set(key, {
        id: `scene-edge:${edgeMap.size}`,
        kind: edge.kind,
        source: edge.source,
        target: edge.target,
        sourceGroup: a,
        targetGroup: b,
        count: 0,
        edgeIds: [],
        relationships: [],
      });
    const group = edgeMap.get(key);
    group.count++;
    group.edgeIds.push(edge.id);
    group.relationships.push({
      id: edge.id,
      source: edge.source,
      target: edge.target,
      label: edge.label,
      sourceStepId: edge.sourceStepId,
      targetStepId: edge.targetStepId,
    });
    edgeGroupForId.set(edge.id, group);
  }
  const edgeGroups = [...edgeMap.values()];
  const edges = edgeGroups.map((group) => [
    group.source,
    group.target,
    {
      faint: group.kind === "peer" || group.kind === "tool",
      kind: group.kind,
      id: group.id,
      count: group.count,
      grouped: group.count > 1,
    },
  ]);
  const script = [];
  const phaseOffsets = [];
  sourcePhases.forEach((phase, index) => {
    const start = index * PHASE_SECONDS;
    const activeIds = phase.activeNodeIds.filter((id) => nodeById.has(id));
    const activeGroups = unique(
      phase.activeEdgeIds.map((id) => edgeGroupForId.get(id)).filter(Boolean),
    );
    const maxAnimated = 24;
    const animated = activeGroups.slice(0, maxAnimated);
    const detail =
      phase.activeEdgeIds.length > animated.length
        ? ` ${phase.activeEdgeIds.length} connections represented by ${animated.length} animated paths.`
        : "";
    script.push({
      t: start + 0.04,
      on: [...activeIds],
      d: 3.6,
      cap: caption(phase) + detail,
      phaseId: phase.id,
      phaseIndex: index,
    });
    const active = new Set(phase.activeEdgeIds);
    animated.forEach((group, edgeIndex) => {
      // A representative must be active in this phase, not just in the same class group.
      const relationship = group.relationships.find((item) =>
        active.has(item.id),
      );
      if (!relationship) return;
      script.push({
        t: start + 0.32 + (edgeIndex % 8) * 0.12,
        msg: [relationship.source, relationship.target],
        dur: 1.05,
        c:
          group.kind === "tool"
            ? "ink"
            : group.kind === "peer"
              ? "mid"
              : "accent",
        phaseId: phase.id,
        phaseIndex: index,
        edgeId: relationship.id,
        representedEdgeIds: group.edgeIds.filter((id) => active.has(id)),
      });
    });
    phaseOffsets.push({
      index,
      id: phase.id,
      start,
      end: start + PHASE_SECONDS,
      title: phase.title,
      kind: phase.kind,
      activeNodeIds: [...activeIds],
      activeEdgeIds: [...phase.activeEdgeIds],
      stepIds: [...phase.stepIds],
      representedEdgeCount: phase.activeEdgeIds.length,
      animatedEdgeCount: animated.length,
    });
  });
  script.sort((a, b) => a.t - b.t);
  return {
    W,
    H,
    nodes,
    edges,
    script,
    duration: Math.max(PHASE_SECONDS, sourcePhases.length * PHASE_SECONDS),
    intro: agents.length
      ? `${agents.length} durable ${agents.length === 1 ? "agent" : "agents"}. A preview of your current proposal.`
      : "Your proposal will take shape here.",
    phaseOffsets,
    phaseStarts: phaseOffsets.map((phase) => phase.start),
    groups,
    profileOnlyClasses: structuredClone(model?.profile_only_classes || []),
    edgeGroups,
    nodeIndex: Object.fromEntries(nodes.map((node) => [node.id, { ...node }])),
    population: {
      count: agents.length,
      renderedCount: agents.length,
      representedCount: agents.length,
      sampled: false,
      groupedEdges: grouped,
      configuredEdgeCount: sourceEdges.length,
      drawnEdgeCount: edges.length,
    },
  };
}

export const createScene = createSimulationScene;

/** Shared drawing and hit-target projection for the automatic preview only. */
export function simulationViewport(scene, width, height) {
  const sc = Math.min(width / scene.W, height / scene.H);
  const small = smallTeamPreview(scene.population?.count, scene.selectedFlowId);
  return small ? {sc, sx: width / scene.W, sy: height / scene.H, ox: 0, oy: 0}
    : {sc, sx: sc, sy: sc, ox: (width - scene.W * sc) / 2, oy: (height - scene.H * sc) / 2};
}

export const SMALL_TEAM_LIMIT = 8;

/** The one rule for a small-team preview: one to eight agents outside a
 *  selected native flow. The stage, layout, projection and renderer all ask it. */
export function smallTeamPreview(agentCount, selectedFlowId) {
  return Number.isSafeInteger(agentCount) && agentCount > 0 && agentCount <= SMALL_TEAM_LIMIT && !selectedFlowId;
}
