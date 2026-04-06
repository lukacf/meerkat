use std::io::Error;

pub use libc::c_int;

#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SigId {
    signal: c_int,
    action: u128,
}

pub const FORBIDDEN: &[c_int] = &[];

pub type Siginfo = ();

pub unsafe fn register<F>(signal: c_int, _action: F) -> Result<SigId, Error>
where
    F: Fn() + Send + Sync + 'static,
{
    Ok(SigId { signal, action: 0 })
}

pub unsafe fn register_sigaction<F>(signal: c_int, _action: F) -> Result<SigId, Error>
where
    F: Fn(&Siginfo) + Send + Sync + 'static,
{
    Ok(SigId { signal, action: 0 })
}

pub unsafe fn register_signal_unchecked<F>(signal: c_int, _action: F) -> Result<SigId, Error>
where
    F: Fn() + Send + Sync + 'static,
{
    Ok(SigId { signal, action: 0 })
}

pub unsafe fn register_unchecked<F>(signal: c_int, _action: F) -> Result<SigId, Error>
where
    F: Fn(&Siginfo) + Send + Sync + 'static,
{
    Ok(SigId { signal, action: 0 })
}

pub fn unregister(_id: SigId) -> bool {
    false
}

pub fn unregister_signal(_signal: c_int) -> bool {
    false
}
