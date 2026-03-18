use meerkat_machine_codegen::render_machine_semantic_model;
use meerkat_machine_schema::catalog::{peer_comms_machine, runtime_ingress_machine};

#[test]
fn machine_semantic_model_keeps_optional_request_id_domains() {
    let rendered = render_machine_semantic_model(&runtime_ingress_machine());

    assert!(rendered.contains("\\E arg_request_id \\in OptionRequestIdValues :"));
    assert!(rendered.contains("\\E arg_reservation_key \\in OptionReservationKeyValues :"));
}

#[test]
fn machine_semantic_model_keeps_optional_domains_in_other_machines() {
    let peer_rendered = render_machine_semantic_model(&peer_comms_machine());

    assert!(peer_rendered.contains("\\E arg_request_id \\in OptionRequestIdValues :"));
    assert!(peer_rendered.contains("\\E arg_reservation_key \\in OptionReservationKeyValues :"));
}
