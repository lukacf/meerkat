// =====================================================================
// Knowledge Base -- Filing Cabinet (typewritten card bestiary)
// =====================================================================

export interface KnowledgeNode {
  id: string;
  label: string;
  type: "person" | "company" | "system" | "date" | "amount" | "event";
}

export interface KnowledgeEdge {
  from: string;
  to: string;
  label: string;
}

const TYPE_LABELS: Record<KnowledgeNode["type"], string> = {
  person: "PERSON",
  company: "COMPANY",
  system: "SYSTEM",
  date: "DATE",
  amount: "AMOUNT",
  event: "EVENT",
};

const nodes = new Map<string, KnowledgeNode>();
const edges: KnowledgeEdge[] = [];
let visible = false;

// -- Public API --

export function addFact(
  subject: string,
  predicate: string,
  object: string,
  subjectType: KnowledgeNode["type"] = "event",
  objectType: KnowledgeNode["type"] = "event",
): void {
  ensureNode(subject, subjectType);
  ensureNode(object, objectType);

  const exists = edges.some(e => e.from === subject && e.to === object && e.label === predicate);
  if (!exists) {
    edges.push({ from: subject, to: object, label: predicate });
  }
}

function ensureNode(id: string, type: KnowledgeNode["type"]): void {
  if (nodes.has(id)) return;
  nodes.set(id, { id, label: id, type });
}

export function getNodeCount(): number { return nodes.size; }

/** Extract knowledge facts from archivist messages (heuristic parsing) */
export function extractFacts(content: string): void {
  const people = content.match(/(?:Alex|Sam|Pat|Casey|Jim|Max|Dev|Robin|Jordan|Morgan|Aria|Scout|Quinn|Bailey|Sage)\s*\w*/g) || [];
  const companies = content.match(/(?:Acme|CloudCorp|Corp)\s*\w*/gi) || [];

  for (const name of people) {
    ensureNode(name.trim(), "person");
  }
  for (const co of companies) {
    ensureNode(co.trim(), "company");
  }

  const amounts = content.match(/\$[\d,]+(?:\.\d{2})?/g) || [];
  for (const amt of amounts) {
    ensureNode(amt, "amount");
  }

  const dates = content.match(/(?:Monday|Tuesday|Wednesday|Thursday|Friday|Saturday|Sunday|today|tomorrow|Q[1-4])/gi) || [];
  for (const d of dates) {
    ensureNode(d, "date");
  }
}

// -- Visibility --

export function isKBVisible(): boolean { return visible; }

export function showKnowledgeBase(contentEl: HTMLElement, footerEl: HTMLElement): void {
  visible = true;
  renderFilingCards(contentEl, footerEl);
}

export function hideKnowledgeBase(): void {
  visible = false;
}

// -- Render filing cards --

function renderFilingCards(contentEl: HTMLElement, footerEl: HTMLElement): void {
  if (nodes.size === 0) {
    contentEl.innerHTML = `<div class="kb-empty">THE FILING CABINET IS EMPTY.<br><br>PROCESS SOME EVENTS AND THE<br>ARCHIVIST WILL STORE KNOWLEDGE HERE.</div>`;
    footerEl.textContent = "0 ENTITIES";
    return;
  }

  // Group nodes by type
  const groups = new Map<KnowledgeNode["type"], KnowledgeNode[]>();
  for (const [, node] of nodes) {
    const list = groups.get(node.type) ?? [];
    list.push(node);
    groups.set(node.type, list);
  }

  // Type display order
  const typeOrder: KnowledgeNode["type"][] = ["person", "company", "system", "event", "date", "amount"];

  let html = "";
  for (const type of typeOrder) {
    const group = groups.get(type);
    if (!group || group.length === 0) continue;

    html += `<div class="kb-section-title">${TYPE_LABELS[type]}S</div>`;

    for (const node of group) {
      // Find all edges involving this node
      const relations = edges
        .filter(e => e.from === node.id || e.to === node.id)
        .map(e => {
          const other = e.from === node.id ? e.to : e.from;
          return e.label ? `${e.label}: ${escapeHtml(other)}` : escapeHtml(other);
        });

      html += `<div class="filing-card">`;
      html += `<span class="card-entity">${escapeHtml(node.label)}</span>`;
      html += `<span class="card-type">${TYPE_LABELS[node.type]}</span>`;

      if (relations.length > 0) {
        html += `<div class="card-relations">`;
        for (const rel of relations) {
          html += `<div class="card-relation">${rel}</div>`;
        }
        html += `</div>`;
      }

      html += `</div>`;
    }
  }

  contentEl.innerHTML = html;
  footerEl.textContent = `${nodes.size} ENTITIES / ${edges.length} RELATIONS`;
}

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
