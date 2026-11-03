// Meerkat #1608 member-kickoff lifecycle notices, one session-history
// system_notice per phase (pending, starting, started, callback_pending,
// failed, cancelled), serialized by meerkat's contract test
// `kickoff_lifecycle_notice_appends_a_lifecycle_comms_notice` (meerkat-runtime
// input.rs): the generated peer-ingress classifier, the comms bridge and the
// runtime append. The sender (peer.id) and payload.peer are both the member
// being kicked off. Live mob payloads also carry `payload.peer_spec`.
export const MEERKAT_1608_KICKOFF_NOTICES = [
  {
    "blocks": [
      {
        "content": [
          {
            "text": "Peer lifecycle notice from peer_id 6f6114cd-2cf7-590f-a172-0e36feacd12c (display_name: incident-command-center/delivery/delivery-lead)\nKind: mob.kickoff_pending\nParams: {\n  \"peer\": \"delivery-lead\",\n  \"role\": \"delivery\"\n}\n\nThis is a one-way status notice, not a request. There is nothing to answer: do not call send_response or send_message for it.",
            "type": "text"
          }
        ],
        "direction": "incoming",
        "intent": "mob.kickoff_pending",
        "kind": "lifecycle",
        "payload": {
          "peer": "delivery-lead",
          "role": "delivery"
        },
        "peer": {
          "display_name": "incident-command-center/delivery/delivery-lead",
          "id": "6f6114cd-2cf7-590f-a172-0e36feacd12c"
        },
        "summary": "Peer lifecycle: mob.kickoff_pending",
        "type": "comms"
      }
    ],
    "body": "Peer lifecycle: mob.kickoff_pending",
    "kind": "comms",
    "role": "system_notice"
  },
  {
    "blocks": [
      {
        "content": [
          {
            "text": "Peer lifecycle notice from peer_id 6f6114cd-2cf7-590f-a172-0e36feacd12c (display_name: incident-command-center/delivery/delivery-lead)\nKind: mob.kickoff_starting\nParams: {\n  \"peer\": \"delivery-lead\",\n  \"role\": \"delivery\"\n}\n\nThis is a one-way status notice, not a request. There is nothing to answer: do not call send_response or send_message for it.",
            "type": "text"
          }
        ],
        "direction": "incoming",
        "intent": "mob.kickoff_starting",
        "kind": "lifecycle",
        "payload": {
          "peer": "delivery-lead",
          "role": "delivery"
        },
        "peer": {
          "display_name": "incident-command-center/delivery/delivery-lead",
          "id": "6f6114cd-2cf7-590f-a172-0e36feacd12c"
        },
        "summary": "Peer lifecycle: mob.kickoff_starting",
        "type": "comms"
      }
    ],
    "body": "Peer lifecycle: mob.kickoff_starting",
    "kind": "comms",
    "role": "system_notice"
  },
  {
    "blocks": [
      {
        "content": [
          {
            "text": "Peer lifecycle notice from peer_id 6f6114cd-2cf7-590f-a172-0e36feacd12c (display_name: incident-command-center/delivery/delivery-lead)\nKind: mob.kickoff_started\nParams: {\n  \"peer\": \"delivery-lead\",\n  \"role\": \"delivery\"\n}\n\nThis is a one-way status notice, not a request. There is nothing to answer: do not call send_response or send_message for it.",
            "type": "text"
          }
        ],
        "direction": "incoming",
        "intent": "mob.kickoff_started",
        "kind": "lifecycle",
        "payload": {
          "peer": "delivery-lead",
          "role": "delivery"
        },
        "peer": {
          "display_name": "incident-command-center/delivery/delivery-lead",
          "id": "6f6114cd-2cf7-590f-a172-0e36feacd12c"
        },
        "summary": "Peer lifecycle: mob.kickoff_started",
        "type": "comms"
      }
    ],
    "body": "Peer lifecycle: mob.kickoff_started",
    "kind": "comms",
    "role": "system_notice"
  },
  {
    "blocks": [
      {
        "content": [
          {
            "text": "Peer lifecycle notice from peer_id 6f6114cd-2cf7-590f-a172-0e36feacd12c (display_name: incident-command-center/delivery/delivery-lead)\nKind: mob.kickoff_callback_pending\nParams: {\n  \"peer\": \"delivery-lead\",\n  \"role\": \"delivery\"\n}\n\nThis is a one-way status notice, not a request. There is nothing to answer: do not call send_response or send_message for it.",
            "type": "text"
          }
        ],
        "direction": "incoming",
        "intent": "mob.kickoff_callback_pending",
        "kind": "lifecycle",
        "payload": {
          "peer": "delivery-lead",
          "role": "delivery"
        },
        "peer": {
          "display_name": "incident-command-center/delivery/delivery-lead",
          "id": "6f6114cd-2cf7-590f-a172-0e36feacd12c"
        },
        "summary": "Peer lifecycle: mob.kickoff_callback_pending",
        "type": "comms"
      }
    ],
    "body": "Peer lifecycle: mob.kickoff_callback_pending",
    "kind": "comms",
    "role": "system_notice"
  },
  {
    "blocks": [
      {
        "content": [
          {
            "text": "Peer lifecycle notice from peer_id 6f6114cd-2cf7-590f-a172-0e36feacd12c (display_name: incident-command-center/delivery/delivery-lead)\nKind: mob.kickoff_failed\nParams: {\n  \"peer\": \"delivery-lead\",\n  \"role\": \"delivery\"\n}\n\nThis is a one-way status notice, not a request. There is nothing to answer: do not call send_response or send_message for it.",
            "type": "text"
          }
        ],
        "direction": "incoming",
        "intent": "mob.kickoff_failed",
        "kind": "lifecycle",
        "payload": {
          "peer": "delivery-lead",
          "role": "delivery"
        },
        "peer": {
          "display_name": "incident-command-center/delivery/delivery-lead",
          "id": "6f6114cd-2cf7-590f-a172-0e36feacd12c"
        },
        "summary": "Peer lifecycle: mob.kickoff_failed",
        "type": "comms"
      }
    ],
    "body": "Peer lifecycle: mob.kickoff_failed",
    "kind": "comms",
    "role": "system_notice"
  },
  {
    "blocks": [
      {
        "content": [
          {
            "text": "Peer lifecycle notice from peer_id 6f6114cd-2cf7-590f-a172-0e36feacd12c (display_name: incident-command-center/delivery/delivery-lead)\nKind: mob.kickoff_cancelled\nParams: {\n  \"peer\": \"delivery-lead\",\n  \"role\": \"delivery\"\n}\n\nThis is a one-way status notice, not a request. There is nothing to answer: do not call send_response or send_message for it.",
            "type": "text"
          }
        ],
        "direction": "incoming",
        "intent": "mob.kickoff_cancelled",
        "kind": "lifecycle",
        "payload": {
          "peer": "delivery-lead",
          "role": "delivery"
        },
        "peer": {
          "display_name": "incident-command-center/delivery/delivery-lead",
          "id": "6f6114cd-2cf7-590f-a172-0e36feacd12c"
        },
        "summary": "Peer lifecycle: mob.kickoff_cancelled",
        "type": "comms"
      }
    ],
    "body": "Peer lifecycle: mob.kickoff_cancelled",
    "kind": "comms",
    "role": "system_notice"
  }
] as const;
