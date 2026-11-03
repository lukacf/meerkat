/** A structural scaffold that can also be serialized into the author's sandbox. */
export function composeTopology(model, options = {}) {
  // Keep all helpers inside this function: api.compose receives its source text.
  const nodes = model?.nodes || [];
  const edges = model?.edges || [];
  const agents = nodes.filter((node) => node.kind === "agent");
  const entry = agents.find((node) => node.id === model.entrypoint) || agents[0];
  const positions = [];
  const groups = [];
  const annotations = [];
  const paths = [];
  const placed = new Map();
  const clamp = (value, low, high) => Math.max(low, Math.min(high, value));
  const spread = (count, low, high) => Array.from({ length: count }, (_, i) =>
    count === 1 ? (low + high) / 2 : low + i * (high - low) / (count - 1));
  const words = (value) => String(value || "").replace(/[_-]+/g, " ").trim();
  const title = (value) => words(value).replace(/\b\w/g, (c) => c.toUpperCase());
  const short = (value, length = 42) => {
    const text = String(value || "").replace(/\s+/g, " ").trim();
    if (text.length <= length) return text;
    const cut = text.slice(0, length - 3);
    const gap = cut.lastIndexOf(" ");
    return `${gap > length * .55 ? cut.slice(0, gap) : cut}...`;
  };
  const put = (node, x, y, labelPos = "below", displayLabel) => {
    const item = { id: node.id, x: clamp(x, 35, 960), y: clamp(y, 40, 515), labelPos,
      displayLabel: short(options.labels?.[node.id] || displayLabel || node.name || node.id) };
    positions.push(item);
    placed.set(node.id, item);
    return item;
  };
  const annotation = (text, x, y, align = "left") => annotations.push({
    text: short(text, 64), x: clamp(x, 35, 960), y: clamp(y, 24, 535), align,
  });
  const classMap = new Map();
  for (const node of agents) {
    const key = node.classId || node.id;
    if (!classMap.has(key)) classMap.set(key, []);
    classMap.get(key).push(node);
  }
  const className = (key, members) => {
    if (options.classLabels?.[key]) return short(options.classLabels[key], 52);
    const stems = members.map((node) => String(node.name || "").replace(/\s+\d+$/, ""));
    return short(stems[0] && stems.every((name) => name === stems[0])
      ? stems[0] : title(key), 52);
  };
  const memberName = (node, key, members) => {
    if (members.length < 2) return node.name;
    const label = String(node.name || node.id);
    const stem = className(key, members);
    // The group heading supplies the role; keep each identity's differentiator.
    const numbered = label.match(/^(.*)\s+(\d+)$/);
    if (numbered && numbered[1] === stem) {
      const noun = stem.replace(/\s+(specialist|assistant|agent|owner)$/i, "");
      return `${noun || stem} ${numbered[2]}`;
    }
    return label;
  };
  const groupFor = (key, members, shape) => {
    const group = { label: className(key, members), memberIds: members.map((node) => node.id), shape };
    groups.push(group);
    return group;
  };
  const byClass = [...classMap].map(([key, members]) => ({ key, members }));
  const peers = edges.filter((edge) => edge.kind === "peer");
  const entryPeers = new Set(peers.flatMap((edge) =>
    edge.source === entry?.id ? [edge.target] : edge.target === entry?.id ? [edge.source] : []));
  const large = agents.length > 48;

  if (entry) put(entry, 520, large ? 165 : 142, "right");

  // The rule of smallTeamPreview in simulation-scene.mjs, written out here because
  // this function's source runs alone in the author's sandbox.
  const smallTeam = agents.length > 0 && agents.length <= 8 && !model?.selectedFlowId;
  if (smallTeam) {
    // A handful of collaborators needs one coherent focal area, not empty lanes.
    const collaborators = agents.filter(node => node.id !== entry?.id);
    if (entry) Object.assign(placed.get(entry.id), {
      x: collaborators.length === 1 ? 460 : 500,
      y: collaborators.length <= 1 ? 240 : 260, labelPos: "above",
    });
    collaborators.forEach((node, index) => {
      if (collaborators.length === 1) put(node, 680, 240, "right");
      else {
        const angle = -Math.PI / 2 + index * Math.PI * 2 / collaborators.length;
        put(node, 500 + Math.cos(angle) * 260, 260 + Math.sin(angle) * 120,
          Math.sin(angle) < -.2 ? "above" : "below");
      }
    });
  } else if (!large) {
    // Classes occupy their own rows. Unrelated singletons share a broad row,
    // ordered by their configured class order, never by incidental DAG order.
    const repeated = byClass.map(({ key, members }) => ({ key, members: members.filter((n) => n.id !== entry?.id) }))
      .filter((group) => group.members.length > 1);
    const singletons = byClass.flatMap(({ members }) => {
      const rest = members.filter((n) => n.id !== entry?.id);
      return rest.length === 1 ? rest : [];
    });
    const rows = [];
    const columnLimit = agents.length <= 20 ? 5 : 8;
    for (const group of repeated) {
      groupFor(group.key, group.members, group.members.length >= 12 ? "cluster" : "lane");
      const chunks = Math.ceil(group.members.length / columnLimit);
      for (let i = 0; i < chunks; i++) rows.push({
        ...group, first: i === 0, members: group.members.slice(i * columnLimit, (i + 1) * columnLimit),
      });
    }
    for (let i = 0; i < singletons.length; i += columnLimit) rows.push({
      key: null, first: false, members: singletons.slice(i, i + columnLimit),
    });
    const rowYs = spread(rows.length, rows.length > 2 ? 221 : 250, 366);
    rows.forEach((row, index) => {
      const y = rowYs[index];
      const xs = spread(row.members.length, row.members.length > 3 ? 160 : 255, row.members.length > 3 ? 895 : 810);
      row.members.forEach((node, i) => put(node, xs[i], y, "below",
        memberName(node, row.key, classMap.get(row.key) || [node])));
      if (row.first) annotation(className(row.key, classMap.get(row.key)).toUpperCase(), 115, y - 34);
    });
  } else {
    const populations = byClass.map(({ key, members }) => ({ key, members: members.filter((node) => node.id !== entry?.id) }))
      .filter((group) => group.members.length);
    const repeated = populations.filter((group) => group.members.length >= 12)
      .sort((a, b) => b.members.length - a.members.length);
    const sparse = populations.filter((group) => group.members.length < 12);
    const ring = (members, x, y, inner, outer) => {
      const count = members.length;
      const ringCount = Math.max(1, Math.ceil(Math.sqrt(count / 11)));
      const radii = spread(ringCount, inner, outer);
      const weight = radii.reduce((sum, radius) => sum + radius, 0);
      let assigned = 0;
      radii.forEach((radius, ri) => {
        const amount = ri === ringCount - 1 ? count - assigned
          : Math.min(count - assigned, Math.max(1, Math.round(count * radius / weight)));
        for (let i = 0; i < amount; i++) {
          const angle = -Math.PI / 2 + (i + ri * .31) * Math.PI * 2 / amount;
          put(members[assigned + i], x + Math.cos(angle) * radius, y + Math.sin(angle) * radius, "below");
        }
        assigned += amount;
      });
    };
    if (repeated.length === 1 && sparse.length === 0 && repeated[0].members.some((node) => entryPeers.has(node.id))) {
      // One owner with one large population reads as a ring around that owner.
      const group = repeated[0];
      Object.assign(placed.get(entry.id), { x: 520, y: 275, labelPos: "right" });
      ring(group.members, 520, 275, 62, 154);
      groupFor(group.key, group.members, "ring");
      annotation(`${className(group.key, group.members)} - ${group.members.length}`, 520, 95, "center");
    } else {
      const maximum = Math.max(1, ...repeated.map((group) => group.members.length));
      repeated.forEach((group, index) => {
        let x, y, radius;
        if (repeated.length <= 3) {
          const slots = repeated.length === 1 ? [[520, 306, 112]]
            : repeated.length === 2 ? [[285, 294, 123], [755, 294, 123]]
              : [[272, 279, 126], [767, 273, 110], [526, 374, 69]];
          [x, y, radius] = slots[index];
          radius = Math.max(radius * .6, radius * Math.sqrt(group.members.length / maximum));
        } else {
          const columns = Math.min(6, Math.ceil(Math.sqrt(repeated.length * 2)));
          const rows = Math.ceil(repeated.length / columns);
          const cellWidth = 820 / columns, cellHeight = 264 / rows;
          x = 110 + cellWidth * (index % columns + .5);
          y = 191 + cellHeight * (Math.floor(index / columns) + .5);
          radius = Math.min(cellWidth / 2 - 14, cellHeight / 2 - 19) * (.58 + .42 * Math.sqrt(group.members.length / maximum));
        }
        ring(group.members, x, y, Math.max(9, radius * .27), radius);
        groupFor(group.key, group.members, "ring");
        annotation(`${className(group.key, group.members)} - ${group.members.length}`, x, y - radius - 19, "center");
      });
      const others = sparse.flatMap((group) => group.members);
      const xs = spread(others.length, 185, 855);
      others.forEach((node, index) => put(node, xs[index], repeated.length > 2 ? 435 : 418, "above"));
    }
  }

  if (smallTeam && options.narrow) {
    if (entry) Object.assign(placed.get(entry.id), {x: 330, y: 242, labelPos: "above"});
    const rest=agents.filter(node=>node.id!==entry?.id);
    rest.forEach((node,index)=>{
      const p=placed.get(node.id);
      if(rest.length===1) Object.assign(p,{x:735,y:345,labelPos:"below"});
      else {
        const a=-Math.PI/2+index*Math.PI*2/rest.length;
        Object.assign(p,{x:500+Math.cos(a)*300,y:280+Math.sin(a)*120,
          labelPos:Math.sin(a)<0?"above":"below"});
      }
    });
  }

  const inputNodes = nodes.filter((node) => node.kind === "input");
  const owner = entry && placed.get(entry.id);
  inputNodes.forEach((node, index) => put(node,
    smallTeam ? options.narrow ? 170 : 145 : (owner?.x || 520) + (index - (inputNodes.length - 1) / 2) * 100,
    smallTeam ? (options.narrow ? 75 : 240) + index * 55 : 48, "above"));
  const workflows = nodes.filter((node) => node.kind === "workflow");
  workflows.forEach((node, index) => put(node, 112 + index * 155, 122, "above", "Workflow run"));
  const outputs = nodes.filter((node) => node.kind === "output");
  outputs.forEach((node, index) => put(node, smallTeam ? options.narrow ? 780 : 855 : 855 + index * 60, smallTeam ? (options.narrow ? 75 : 110) + index * 55 : 142, "above",
    node.name === "Workflow result" ? "Result" : node.name));

  const memories = nodes.filter((node) => ["knowledge", "notes"].includes(node.kind));
  const resources = nodes.filter((node) => !["agent", "input", "workflow", "output", "knowledge", "notes"].includes(node.kind));
  const resourceAnchor = (node) => {
    const owners = edges.filter((edge) => edge.kind === "tool" && edge.target === node.id)
      .map((edge) => placed.get(edge.source)?.x).filter(Number.isFinite);
    return owners.length ? owners.reduce((sum, x) => sum + x, 0) / owners.length : 520;
  };
  resources.sort((a, b) => resourceAnchor(a) - resourceAnchor(b));
  const resourceRows = Math.ceil(resources.length / 5);
  resources.forEach((node, index) => {
    const row = Math.floor(index / 5);
    const count = Math.min(5, resources.length - row * 5);
    const x = spread(count, 180, 890)[index % 5];
    put(node, x, resourceRows > 1 ? (smallTeam ? 420 : 448) + row * Math.min(28, 47 / (resourceRows - 1)) : smallTeam ? 414 : 453, "below");
  });
  if (resources.length && !smallTeam) annotation("TOOLS & SOURCES", 60, 429);
  memories.forEach((node, index) => put(node, spread(memories.length, 360, 715)[index], smallTeam ? options.narrow ? 482 : 408 : 510, "below"));
  if (memories.length && !smallTeam) annotation("SHARED", 60, 510);

  // Unexpected future node kinds must still survive a scaffold revision.
  nodes.filter((node) => !placed.has(node.id)).forEach((node, index) => put(node, 80 + index * 50, 480));

  for (const edge of edges) {
    const a = placed.get(edge.source), b = placed.get(edge.target);
    if (!a || !b) continue;
    let points = [{ x: a.x, y: a.y }];
    if (!smallTeam && !large && edge.kind !== "tool" && Math.abs(a.x - b.x) > 2 && Math.abs(a.y - b.y) > 2) {
      const upper = Math.min(a.y, b.y), lower = Math.max(a.y, b.y);
      const bus = upper < 180 && lower > 210 ? lower - 42 : (upper + lower) / 2;
      points.push({ x: a.x, y: bus }, { x: b.x, y: bus });
    } else if (!smallTeam && !large && ["peer", "flow"].includes(edge.kind) && Math.abs(a.y - b.y) < 2 && Math.abs(a.x - b.x) > 2) {
      points.push({ x: a.x, y: a.y - 27 }, { x: b.x, y: b.y - 27 });
    }
    points.push({ x: b.x, y: b.y });
    points = points.filter((point, index) => index === 0 || point.x !== points[index - 1].x || point.y !== points[index - 1].y);
    if (points.length < 2) points.push({ ...points[0] });
    paths.push({ edgeId: edge.id, points });
  }
  for (const item of options.annotations || []) {
    if (typeof item.text === "string" && Number.isFinite(item.x) && Number.isFinite(item.y))
      annotation(item.text, item.x, item.y, ["left", "center", "right"].includes(item.align) ? item.align : "left");
  }
  return { positions, groups, annotations, paths };
}
