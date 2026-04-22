use crate::MachineSchema;

pub fn loop_iteration_machine() -> MachineSchema {
    let mut schema = crate::compat::loop_iteration_machine();
    schema.rust.module = "runtime::flow_kernels::loop_iteration".into();
    schema
}
