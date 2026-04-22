use crate::MachineSchema;

pub fn flow_frame_machine() -> MachineSchema {
    let mut schema = crate::compat::flow_frame_machine();
    schema.rust.module = "runtime::flow_kernels::flow_frame".into();
    schema
}
