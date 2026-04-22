pub mod flow_frame_machine;
pub mod flow_run_machine;
pub mod loop_iteration_machine;

use crate::MachineSchema;

pub fn flow_frame_machine() -> MachineSchema {
    flow_frame_machine::flow_frame_machine()
}

pub fn flow_run_machine() -> MachineSchema {
    flow_run_machine::flow_run_machine()
}

pub fn loop_iteration_machine() -> MachineSchema {
    loop_iteration_machine::loop_iteration_machine()
}
