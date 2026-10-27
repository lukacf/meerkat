// Exact selected query constants from the retained Activepieces source.
export const ISSUE_SELECTION = `
  id
  identifier
  number
  title
  description
  url
  priority
  priorityLabel
  estimate
  dueDate
  branchName
  createdAt
  updatedAt
  startedAt
  completedAt
  canceledAt
  archivedAt
  trashed
  state { id name type }
  team { id key name }
  assignee { id name email }
  creator { id name email }
  project { id name }
  projectMilestone { id name }
  cycle { id number name }
  parent { id identifier title }
  labels(first: 20) {
    pageInfo { hasNextPage endCursor }
    nodes { id name }
  }
`;

export const ISSUE_LABELS_PAGE_QUERY = `
  query LinearIssueLabelsPage($id: String!, $first: Int!, $filter: IssueLabelFilter) {
    issue(id: $id) {
      labels(first: $first, filter: $filter) {
        pageInfo { hasNextPage endCursor }
        nodes { id name }
      }
    }
  }
`;

export const GET_ISSUE_QUERY = `
  query LinearGetIssue($id: String!) {
    issue(id: $id) { ${ISSUE_SELECTION} }
  }
`;
