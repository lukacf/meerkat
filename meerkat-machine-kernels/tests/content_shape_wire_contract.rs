#![allow(clippy::expect_used)]

use meerkat_core::turn_execution_authority::ContentShape as CoreContentShape;
use meerkat_machine_kernels::generated::meerkat::ContentShape as KernelContentShape;

fn shapes() -> [(CoreContentShape, KernelContentShape); 6] {
    [
        (
            CoreContentShape::Conversation,
            KernelContentShape::Conversation,
        ),
        (
            CoreContentShape::ConversationAndContext,
            KernelContentShape::ConversationAndContext,
        ),
        (CoreContentShape::Context, KernelContentShape::Context),
        (CoreContentShape::Empty, KernelContentShape::Empty),
        (
            CoreContentShape::ImmediateAppend,
            KernelContentShape::ImmediateAppend,
        ),
        (
            CoreContentShape::ImmediateContext,
            KernelContentShape::ImmediateContext,
        ),
    ]
}

#[test]
fn generated_meerkat_content_shape_uses_core_wire_labels() {
    for (core_shape, kernel_shape) in shapes() {
        assert_eq!(kernel_shape.as_str(), core_shape.as_str());
        assert_eq!(kernel_shape.to_string(), core_shape.as_str());

        let encoded = serde_json::to_string(&kernel_shape).expect("serialize content shape");
        let expected = serde_json::to_string(core_shape.as_str()).expect("serialize core label");
        assert_eq!(encoded, expected);

        let decoded: KernelContentShape =
            serde_json::from_str(&encoded).expect("deserialize content shape");
        assert_eq!(decoded, kernel_shape);
    }
}
