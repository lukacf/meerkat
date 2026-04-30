#![allow(clippy::expect_used)]

use meerkat_core::turn_execution_authority::ContentShape as CoreContentShape;
use meerkat_runtime::meerkat_machine::dsl::ContentShape as RuntimeContentShape;

fn core_shapes() -> [CoreContentShape; 6] {
    [
        CoreContentShape::Conversation,
        CoreContentShape::ConversationAndContext,
        CoreContentShape::Context,
        CoreContentShape::Empty,
        CoreContentShape::ImmediateAppend,
        CoreContentShape::ImmediateContext,
    ]
}

#[test]
fn runtime_dsl_content_shape_round_trips_against_core_contract() {
    for core_shape in core_shapes() {
        let runtime_shape = RuntimeContentShape::from(core_shape);
        assert_eq!(runtime_shape.as_str(), core_shape.as_str());
        assert_eq!(CoreContentShape::from(runtime_shape), core_shape);
    }
}

#[test]
fn runtime_dsl_content_shape_labels_route_through_core_contract() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("meerkat_machine")
            .join("dsl.rs"),
    )
    .expect("read runtime DSL source");

    assert!(
        source.contains(
            "meerkat_core::turn_execution_authority::ContentShape::Conversation.as_str()"
        ),
        "runtime DSL ContentShape::as_str must route labels through the shared core contract"
    );
    assert!(
        !source.contains("Self::Conversation => \"conversation\""),
        "runtime DSL ContentShape must not carry a local content-shape label table"
    );
}
