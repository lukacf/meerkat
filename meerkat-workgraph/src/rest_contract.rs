#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkGraphRestRoute {
    Items,
    Item,
    Ready,
    Snapshot,
    Events,
    GoalCreate,
    GoalStatus,
    GoalConfirm,
    GoalRequestClose,
    AttentionList,
    AttentionPause,
    AttentionResume,
    AttentionContinue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkGraphRestOperationDescriptor {
    pub method: &'static str,
    pub summary: &'static str,
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
        response_schema: "WorkGraphItemsResponse",
    }];

const WORKGRAPH_ITEM_OPERATIONS: &[WorkGraphRestOperationDescriptor] =
    &[WorkGraphRestOperationDescriptor {
        method: "get",
        summary: "Get WorkGraph item",
        response_schema: "WorkItem",
    }];

const WORKGRAPH_READY_OPERATIONS: &[WorkGraphRestOperationDescriptor] =
    &[WorkGraphRestOperationDescriptor {
        method: "get",
        summary: "List ready WorkGraph items",
        response_schema: "WorkGraphItemsResponse",
    }];

const WORKGRAPH_SNAPSHOT_OPERATIONS: &[WorkGraphRestOperationDescriptor] =
    &[WorkGraphRestOperationDescriptor {
        method: "get",
        summary: "Read WorkGraph snapshot",
        response_schema: "WorkGraphSnapshot",
    }];

const WORKGRAPH_EVENTS_OPERATIONS: &[WorkGraphRestOperationDescriptor] =
    &[WorkGraphRestOperationDescriptor {
        method: "get",
        summary: "List WorkGraph events",
        response_schema: "WorkGraphEventsResponse",
    }];

const WORKGRAPH_GOAL_CREATE_OPERATIONS: &[WorkGraphRestOperationDescriptor] =
    &[WorkGraphRestOperationDescriptor {
        method: "post",
        summary: "Create WorkGraph goal and attention binding",
        response_schema: "GoalCreateResult",
    }];

const WORKGRAPH_GOAL_STATUS_OPERATIONS: &[WorkGraphRestOperationDescriptor] =
    &[WorkGraphRestOperationDescriptor {
        method: "post",
        summary: "Read WorkGraph goal item and attention status",
        response_schema: "GoalStatusResult",
    }];

const WORKGRAPH_GOAL_CONFIRM_OPERATIONS: &[WorkGraphRestOperationDescriptor] =
    &[WorkGraphRestOperationDescriptor {
        method: "post",
        summary: "Attach WorkGraph goal confirmation evidence",
        response_schema: "GoalConfirmResult",
    }];

const WORKGRAPH_GOAL_REQUEST_CLOSE_OPERATIONS: &[WorkGraphRestOperationDescriptor] =
    &[WorkGraphRestOperationDescriptor {
        method: "post",
        summary: "Request policy-gated WorkGraph goal closure",
        response_schema: "GoalRequestCloseResult",
    }];

const WORKGRAPH_ATTENTION_LIST_OPERATIONS: &[WorkGraphRestOperationDescriptor] =
    &[WorkGraphRestOperationDescriptor {
        method: "post",
        summary: "List WorkGraph attention bindings",
        response_schema: "AttentionListResult",
    }];

const WORKGRAPH_ATTENTION_PAUSE_OPERATIONS: &[WorkGraphRestOperationDescriptor] =
    &[WorkGraphRestOperationDescriptor {
        method: "post",
        summary: "Pause WorkGraph attention binding",
        response_schema: "AttentionBindingResult",
    }];

const WORKGRAPH_ATTENTION_RESUME_OPERATIONS: &[WorkGraphRestOperationDescriptor] =
    &[WorkGraphRestOperationDescriptor {
        method: "post",
        summary: "Resume WorkGraph attention binding",
        response_schema: "AttentionBindingResult",
    }];

const WORKGRAPH_ATTENTION_CONTINUE_OPERATIONS: &[WorkGraphRestOperationDescriptor] =
    &[WorkGraphRestOperationDescriptor {
        method: "post",
        summary: "Queue WorkGraph attention continuation for its target session",
        response_schema: "AttentionContinueResult",
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
        route: WorkGraphRestRoute::GoalCreate,
        path: "/workgraph/goal/create",
        operations: WORKGRAPH_GOAL_CREATE_OPERATIONS,
    },
    WorkGraphRestPathDescriptor {
        route: WorkGraphRestRoute::GoalStatus,
        path: "/workgraph/goal/status",
        operations: WORKGRAPH_GOAL_STATUS_OPERATIONS,
    },
    WorkGraphRestPathDescriptor {
        route: WorkGraphRestRoute::GoalConfirm,
        path: "/workgraph/goal/confirm",
        operations: WORKGRAPH_GOAL_CONFIRM_OPERATIONS,
    },
    WorkGraphRestPathDescriptor {
        route: WorkGraphRestRoute::GoalRequestClose,
        path: "/workgraph/goal/request_close",
        operations: WORKGRAPH_GOAL_REQUEST_CLOSE_OPERATIONS,
    },
    WorkGraphRestPathDescriptor {
        route: WorkGraphRestRoute::AttentionList,
        path: "/workgraph/attention/list",
        operations: WORKGRAPH_ATTENTION_LIST_OPERATIONS,
    },
    WorkGraphRestPathDescriptor {
        route: WorkGraphRestRoute::AttentionPause,
        path: "/workgraph/attention/pause",
        operations: WORKGRAPH_ATTENTION_PAUSE_OPERATIONS,
    },
    WorkGraphRestPathDescriptor {
        route: WorkGraphRestRoute::AttentionResume,
        path: "/workgraph/attention/resume",
        operations: WORKGRAPH_ATTENTION_RESUME_OPERATIONS,
    },
    WorkGraphRestPathDescriptor {
        route: WorkGraphRestRoute::AttentionContinue,
        path: "/workgraph/attention/continue",
        operations: WORKGRAPH_ATTENTION_CONTINUE_OPERATIONS,
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
