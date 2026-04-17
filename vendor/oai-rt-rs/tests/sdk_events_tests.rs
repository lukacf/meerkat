use oai_rt_rs::protocol::server_events::ServerEvent;
use oai_rt_rs::sdk::events::SdkEvent;

#[test]
fn sdk_event_maps_text_delta() {
    let evt = ServerEvent::ResponseOutputTextDelta {
        event_id: "evt_1".to_string(),
        response_id: "resp_1".to_string(),
        item_id: "item_1".to_string(),
        output_index: 0,
        content_index: 0,
        delta: "hi".to_string(),
    };

    let mapped = SdkEvent::from_server(evt).expect("event maps");
    match mapped {
        SdkEvent::TextDelta {
            response_id,
            item_id,
            delta,
            ..
        } => {
            assert_eq!(response_id, "resp_1");
            assert_eq!(item_id, "item_1");
            assert_eq!(delta, "hi");
        }
        other => panic!("unexpected mapping: {other:?}"),
    }
}
