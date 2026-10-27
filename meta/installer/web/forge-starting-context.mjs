export const startingAudiences = [
  { id: "personal", label: "Just me", description: "Personal projects or everyday work", context: "For myself, " },
  { id: "household", label: "My household", description: "Things we share at home", context: "For my household, " },
  { id: "team", label: "A team", description: "Work shared by a group", context: "For a team, " },
  { id: "many", label: "Many people", description: "Customers, members, or an organization", context: "For many people, " },
  { id: "exploring", label: "I'm still exploring", description: "Help me find a starting point", context: "I am still exploring who this could help. " },
];
export const startingKinds = [
  { id: "organizing", label: "Keeping track of things", description: "Information, plans, or commitments", context: "I would like help organizing information and commitments.", prompt: "What's one thing you need to keep track of?", example: "Keep track of decisions and deadlines for a project." },
  { id: "understanding", label: "Figuring something out", description: "Research, compare, or reach a decision", context: "I would like help researching and understanding something.", prompt: "What question would you like help answering?", example: "Compare two options and explain which one fits my needs." },
  { id: "repetitive", label: "Repetitive work", description: "The same manual steps, again and again", context: "I would like help with repetitive work.", prompt: "What's one task you keep doing by hand?", example: "Turn scattered notes into a clear weekly update." },
  { id: "other", label: "Something else", description: "Something you'd describe in your own words", context: "I have something else in mind.", prompt: "What would a useful result look like?", example: "Help me plan a project and keep the next steps clear." },
  { id: "exploring", label: "Still exploring", description: "Start with a frustration or an unfinished idea", context: "I am still exploring what kind of help would be useful.", prompt: "What took more time than you'd like this week?", example: "Each Friday, I spend an hour finding the updates I need." },
];

/** Choices are context metadata; the composer remains the user's own writing. */
export function startingContextDetails(hint) {
  if (!hint || typeof hint !== "object" || Array.isArray(hint) ||
      Object.keys(hint).length !== 3 || hint.version !== 2 ||
      !["version", "audience_id", "kind_id"].every(key => Object.hasOwn(hint, key)) ||
      typeof hint.audience_id !== "string" || typeof hint.kind_id !== "string") return null;
  const audience = startingAudiences.find(option => option.id === hint.audience_id);
  const kind = startingKinds.find(option => option.id === hint.kind_id);
  return audience && kind ? { audience, kind } : null;
}

export function startingContextText(hint) {
  const details = startingContextDetails(hint);
  return details ? `${details.audience.context}${details.kind.context}` : "";
}
