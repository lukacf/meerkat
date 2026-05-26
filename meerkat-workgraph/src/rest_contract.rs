#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkGraphRestRoute {
    Items,
    Item,
    Ready,
    Snapshot,
    Events,
    GoalStatus,
    AttentionList,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkGraphRestOperationDescriptor {
    pub method: &'static str,
    pub summary: &'static str,
    pub request_schema: Option<&'static str>,
    pub response_schema: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkGraphRestPathDescriptor {
    pub route: WorkGraphRestRoute,
    pub path: &'static str,
    pub operations: &'static [WorkGraphRestOperationDescriptor],
}

const WORKGRAPH_ITEMS_OPERATIONS: &[WorkGraphRestOperationDescriptor] =
    &[WorkGraphRestOperationDescriptor {
        method: "get",
        summary: "List WorkGraph items",
        request_schema: None,
        response_schema: "WorkGraphItemsResponse",
    }];

const WORKGRAPH_ITEM_OPERATIONS: &[WorkGraphRestOperationDescriptor] =
    &[WorkGraphRestOperationDescriptor {
        method: "get",
        summary: "Get WorkGraph item",
        request_schema: None,
        response_schema: "WorkItem",
    }];

const WORKGRAPH_READY_OPERATIONS: &[WorkGraphRestOperationDescriptor] =
    &[WorkGraphRestOperationDescriptor {
        method: "get",
        summary: "List ready WorkGraph items",
        request_schema: None,
        response_schema: "WorkGraphItemsResponse",
    }];

const WORKGRAPH_SNAPSHOT_OPERATIONS: &[WorkGraphRestOperationDescriptor] =
    &[WorkGraphRestOperationDescriptor {
        method: "get",
        summary: "Read WorkGraph snapshot",
        request_schema: None,
        response_schema: "WorkGraphSnapshot",
    }];

const WORKGRAPH_EVENTS_OPERATIONS: &[WorkGraphRestOperationDescriptor] =
    &[WorkGraphRestOperationDescriptor {
        method: "get",
        summary: "List WorkGraph events",
        request_schema: None,
        response_schema: "WorkGraphEventsResponse",
    }];

const WORKGRAPH_GOAL_STATUS_OPERATIONS: &[WorkGraphRestOperationDescriptor] =
    &[WorkGraphRestOperationDescriptor {
        method: "post",
        summary: "Read WorkGraph goal item and attention status",
        request_schema: Some("GoalStatusRequest"),
        response_schema: "GoalStatusResult",
    }];

const WORKGRAPH_ATTENTION_LIST_OPERATIONS: &[WorkGraphRestOperationDescriptor] =
    &[WorkGraphRestOperationDescriptor {
        method: "post",
        summary: "List WorkGraph attention bindings",
        request_schema: Some("AttentionListRequest"),
        response_schema: "AttentionListResult",
    }];

pub const WORKGRAPH_REST_PATHS: &[WorkGraphRestPathDescriptor] = &[
    WorkGraphRestPathDescriptor {
        route: WorkGraphRestRoute::Items,
        path: "/workgraph/items",
        operations: WORKGRAPH_ITEMS_OPERATIONS,
    },
    WorkGraphRestPathDescriptor {
        route: WorkGraphRestRoute::Item,
        path: "/workgraph/items/{id}",
        operations: WORKGRAPH_ITEM_OPERATIONS,
    },
    WorkGraphRestPathDescriptor {
        route: WorkGraphRestRoute::Ready,
        path: "/workgraph/ready",
        operations: WORKGRAPH_READY_OPERATIONS,
    },
    WorkGraphRestPathDescriptor {
        route: WorkGraphRestRoute::Snapshot,
        path: "/workgraph/snapshot",
        operations: WORKGRAPH_SNAPSHOT_OPERATIONS,
    },
    WorkGraphRestPathDescriptor {
        route: WorkGraphRestRoute::Events,
        path: "/workgraph/events",
        operations: WORKGRAPH_EVENTS_OPERATIONS,
    },
    WorkGraphRestPathDescriptor {
        route: WorkGraphRestRoute::GoalStatus,
        path: "/workgraph/goal/status",
        operations: WORKGRAPH_GOAL_STATUS_OPERATIONS,
    },
    WorkGraphRestPathDescriptor {
        route: WorkGraphRestRoute::AttentionList,
        path: "/workgraph/attention/list",
        operations: WORKGRAPH_ATTENTION_LIST_OPERATIONS,
    },
];

pub fn workgraph_rest_path_catalog() -> &'static [WorkGraphRestPathDescriptor] {
    WORKGRAPH_REST_PATHS
}

pub fn workgraph_rest_response_schema(path: &str, method: &str) -> Option<&'static str> {
    WORKGRAPH_REST_PATHS
        .iter()
        .find(|entry| entry.path == path)
        .and_then(|entry| {
            entry
                .operations
                .iter()
                .find(|operation| operation.method == method)
        })
        .map(|operation| operation.response_schema)
}

pub fn workgraph_rest_request_response_schema(
    path: &str,
    method: &str,
) -> Option<(Option<&'static str>, &'static str)> {
    WORKGRAPH_REST_PATHS
        .iter()
        .find(|entry| entry.path == path)
        .and_then(|entry| {
            entry
                .operations
                .iter()
                .find(|operation| operation.method == method)
        })
        .map(|operation| (operation.request_schema, operation.response_schema))
}
