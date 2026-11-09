//! Frozen pre-cutover JSON, assembled from the captured old codecs.
//! These are literal historical-format vectors, not a runtime database capture.
//! See the evidence stage IMPLEMENTATION.md for source hashes and provenance.

pub(super) const PENDING_ADMISSION: &str = r#"{
  "admission_key": "42f89608ddd5f38a09b46b35d24eeb05de5ad59ba40cd19404e6ae0e1a083b22",
  "data": {
    "body": {
      "continuation_receipt": null,
      "detail": null,
      "ready": null,
      "request": {
        "authority": {
          "ceiling": {
            "actions": [
              {
                "action": "find_and_install",
                "resource": "agent-a"
              }
            ],
            "frontends": [],
            "output_audiences": [
              "person-a"
            ],
            "services": []
          },
          "deployment": "deployment-a",
          "expires_at_ms": 1000,
          "hops": 0,
          "issued_at_ms": 100,
          "issuer": "host",
          "max_hops": 3,
          "origin": "private-a",
          "parent_digest": null,
          "policy_revision": "policy-1",
          "receiver": "agent-a",
          "requester": "person-a",
          "tenant": "tenant-a"
        },
        "authority_digest": "34903c1ba1c7e1cf8c5255f5f372a1a506bbff92a3b75139627767ff5be897d3",
        "correlation_digest": "42f89608ddd5f38a09b46b35d24eeb05de5ad59ba40cd19404e6ae0e1a083b22",
        "host": {
          "continuation": {
            "audience": [
              "person-a"
            ],
            "delivery_key": "delivery-1",
            "owner": "runtime",
            "revision": "2",
            "task": "original-task"
          },
          "correlation": "original-call-1",
          "environment_revision": "environment-1",
          "limits": {
            "deadline_ms": 900,
            "max_effect_attempts": 5,
            "max_workers": 3
          },
          "parent_operation": "original-task",
          "target": "agent-a"
        },
        "input_digest": "d143a2d4363b42068aeeced187b07426a1082018bf80dd18af1458d03646cfdb",
        "request": {
          "caller_observations": [
            "No matching tools."
          ],
          "request": "Frozen old setup request.",
          "user_context": [
            {
              "content_digest": "e98f7a2ad74364bdbf2595692fe7526ae159022fec96a8a537571d8ab11715e3",
              "id": "original-context",
              "owner": "evidence-owner",
              "revision": "1"
            }
          ]
        }
      },
      "selections": [],
      "state": {
        "admitted_at": 200,
        "authority_digest": "34903c1ba1c7e1cf8c5255f5f372a1a506bbff92a3b75139627767ff5be897d3",
        "continuation_id": "640cf2b2a15e4be8bda25b61c1223a992bf209837595530643f931f2a7ebc3a6",
        "continuation_receipt_ref": "",
        "deadline": 900,
        "decision_ref": "",
        "detail_ref": "",
        "last_observed_at": 200,
        "lifecycle_phase": "investigating",
        "operation_id": "setup-42f89608ddd5f38a09b46b35d24eeb05de5ad59ba40cd19404e6ae0e1a083b22",
        "readiness_digest": "",
        "readiness_observed_at": 0,
        "readiness_ref": "",
        "request_digest": "d143a2d4363b42068aeeced187b07426a1082018bf80dd18af1458d03646cfdb",
        "revision": 1,
        "selection_digest": "",
        "selection_generation": 0,
        "terminal_at": 0,
        "terminal_revision": 0
      }
    },
    "schema": "meerkat_toolkit.acquisition",
    "version": 2
  },
  "effects": [
    {
      "id": "work-admission",
      "idempotency_key": "setup-42f89608ddd5f38a09b46b35d24eeb05de5ad59ba40cd19404e6ae0e1a083b22:work-admission",
      "kind": "work_admission_requested",
      "replay_safety": "Idempotent",
      "request": {
        "body": {
          "authority_digest": "34903c1ba1c7e1cf8c5255f5f372a1a506bbff92a3b75139627767ff5be897d3",
          "kind": "work_admission_requested",
          "operation_id": "setup-42f89608ddd5f38a09b46b35d24eeb05de5ad59ba40cd19404e6ae0e1a083b22",
          "request_digest": "d143a2d4363b42068aeeced187b07426a1082018bf80dd18af1458d03646cfdb"
        },
        "schema": "meerkat_toolkit.work_admission_requested",
        "version": 1
      }
    }
  ],
  "events": [
    {
      "data": {
        "body": {
          "authority_digest": "34903c1ba1c7e1cf8c5255f5f372a1a506bbff92a3b75139627767ff5be897d3",
          "kind": "work_admission_requested",
          "operation_id": "setup-42f89608ddd5f38a09b46b35d24eeb05de5ad59ba40cd19404e6ae0e1a083b22",
          "request_digest": "d143a2d4363b42068aeeced187b07426a1082018bf80dd18af1458d03646cfdb"
        },
        "schema": "meerkat_toolkit.work_admission_requested",
        "version": 1
      },
      "kind": "work_admission_requested"
    }
  ],
  "key": {
    "id": "setup-42f89608ddd5f38a09b46b35d24eeb05de5ad59ba40cd19404e6ae0e1a083b22",
    "owner": "toolkit/acquisition/deployment-a",
    "realm": "tenant-a"
  }
}"#;

pub(super) const PREPARED_ADMISSION: &str = r#"{
  "admission_key": "setup-42f89608ddd5f38a09b46b35d24eeb05de5ad59ba40cd19404e6ae0e1a083b22:work-admission",
  "data": {
    "body": {
      "acquisition": {
        "id": "setup-42f89608ddd5f38a09b46b35d24eeb05de5ad59ba40cd19404e6ae0e1a083b22",
        "owner": "toolkit/acquisition/deployment-a",
        "realm": "tenant-a"
      },
      "effect_digest": "74c9d9c134cd4ed8955466365b0424a918f1db71f6febfc9977ce8ff4a8378b0",
      "environment_revision": "environment-1",
      "input_reference": {
        "content_digest": "241b18706eeff56b7da1d14f35ea885eba27048143e03af9a2ebbcdd8e9e35d5",
        "id": "canonical-original-request",
        "owner": "evidence-owner",
        "revision": "1"
      },
      "namespace": "setup",
      "realm": "tenant-a",
      "request": {
        "cancelled_child_join_policy": "require_success",
        "completion_policy": {
          "kind": "self_attest"
        },
        "failed_child_join_policy": "require_success",
        "priority": "medium",
        "title": "Frozen configured setup worker"
      }
    },
    "schema": "meerkat_toolkit.prepared_setup_work",
    "version": 1
  },
  "effects": [],
  "events": [],
  "key": {
    "id": "setup-42f89608ddd5f38a09b46b35d24eeb05de5ad59ba40cd19404e6ae0e1a083b22",
    "owner": "toolkit/prepared-work/deployment-a",
    "realm": "tenant-a"
  }
}"#;

pub(super) const TERMINAL_RECORD: &str = r#"{
  "body": {
    "continuation_receipt": null,
    "detail": {
      "content_digest": "8b47045eb7b8ca42aa967f33ee1d014ba89f8d1ac207426b482d34b5c0d90935",
      "id": "cancelled",
      "owner": "evidence-owner",
      "revision": "1"
    },
    "ready": null,
    "request": {
      "authority": {
        "ceiling": {
          "actions": [
            {
              "action": "find_and_install",
              "resource": "agent-a"
            }
          ],
          "frontends": [],
          "output_audiences": [
            "person-a"
          ],
          "services": []
        },
        "deployment": "deployment-a",
        "expires_at_ms": 1000,
        "hops": 0,
        "issued_at_ms": 100,
        "issuer": "host",
        "max_hops": 3,
        "origin": "private-a",
        "parent_digest": null,
        "policy_revision": "policy-1",
        "receiver": "agent-a",
        "requester": "person-a",
        "tenant": "tenant-a"
      },
      "authority_digest": "34903c1ba1c7e1cf8c5255f5f372a1a506bbff92a3b75139627767ff5be897d3",
      "correlation_digest": "42f89608ddd5f38a09b46b35d24eeb05de5ad59ba40cd19404e6ae0e1a083b22",
      "host": {
        "continuation": {
          "audience": [
            "person-a"
          ],
          "delivery_key": "delivery-1",
          "owner": "runtime",
          "revision": "2",
          "task": "original-task"
        },
        "correlation": "original-call-1",
        "environment_revision": "environment-1",
        "limits": {
          "deadline_ms": 900,
          "max_effect_attempts": 5,
          "max_workers": 3
        },
        "parent_operation": "original-task",
        "target": "agent-a"
      },
      "input_digest": "d143a2d4363b42068aeeced187b07426a1082018bf80dd18af1458d03646cfdb",
      "request": {
        "caller_observations": [
          "No matching tools."
        ],
        "request": "Frozen old setup request.",
        "user_context": [
          {
            "content_digest": "e98f7a2ad74364bdbf2595692fe7526ae159022fec96a8a537571d8ab11715e3",
            "id": "original-context",
            "owner": "evidence-owner",
            "revision": "1"
          }
        ]
      }
    },
    "selections": [],
    "state": {
      "admitted_at": 200,
      "authority_digest": "34903c1ba1c7e1cf8c5255f5f372a1a506bbff92a3b75139627767ff5be897d3",
      "continuation_id": "640cf2b2a15e4be8bda25b61c1223a992bf209837595530643f931f2a7ebc3a6",
      "continuation_receipt_ref": "",
      "deadline": 900,
      "decision_ref": "",
      "detail_ref": "336b473cf4ec5f10e2fecf67de98b8deaec3e107db2a2dfa9f3797947208e871",
      "last_observed_at": 220,
      "lifecycle_phase": "cancelled",
      "operation_id": "setup-42f89608ddd5f38a09b46b35d24eeb05de5ad59ba40cd19404e6ae0e1a083b22",
      "readiness_digest": "",
      "readiness_observed_at": 0,
      "readiness_ref": "",
      "request_digest": "d143a2d4363b42068aeeced187b07426a1082018bf80dd18af1458d03646cfdb",
      "revision": 2,
      "selection_digest": "",
      "selection_generation": 0,
      "terminal_at": 220,
      "terminal_revision": 2
    }
  },
  "schema": "meerkat_toolkit.acquisition",
  "version": 2
}"#;

pub(super) const CONTINUATION_EFFECT: &str = r#"{
  "id": "continuation-2",
  "idempotency_key": "setup-42f89608ddd5f38a09b46b35d24eeb05de5ad59ba40cd19404e6ae0e1a083b22:continuation-2",
  "kind": "continuation_requested",
  "replay_safety": "ReconcileBeforeRetry",
  "request": {
    "body": {
      "continuation_id": "640cf2b2a15e4be8bda25b61c1223a992bf209837595530643f931f2a7ebc3a6",
      "kind": "continuation_requested",
      "operation_id": "setup-42f89608ddd5f38a09b46b35d24eeb05de5ad59ba40cd19404e6ae0e1a083b22",
      "outcome": "cancelled",
      "terminal_revision": 2
    },
    "schema": "meerkat_toolkit.continuation_requested",
    "version": 1
  }
}"#;

pub(super) const UNCERTAIN_OBSERVATION: &str = r#"{
  "body": {
    "delivery_identity": "e503c0052c914707ee335ea57fd6b4a3df7dcc0e031bafbb579b2c45867d85ba",
    "kind": {
      "kind": "ambiguous"
    },
    "obligation_digest": "9bee0a9a20d34ff33b4f560931f8271d61f6c91c8953ae4fd860fc08d3c75737"
  },
  "schema": "meerkat_toolkit.continuation_observation",
  "version": 1
}"#;
