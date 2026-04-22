use crate::MachineSchema;

pub fn flow_run_machine() -> MachineSchema {
    let mut schema = crate::compat::flow_run_machine();
    schema.rust.module = "runtime::flow_kernels::flow_run".into();
    schema
}
