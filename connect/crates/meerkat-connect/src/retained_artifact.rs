//! Bounded retained-artifact reads shared by setup and approval owners.

use std::path::Path;

pub(crate) fn read_regular_artifact(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    if !path.is_absolute() || path.is_symlink() || !path.is_file() {
        return Err("retained compiler artifact must be an absolute regular file".into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|error| error.to_string())?
        .take(256 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > 256 * 1024 {
        return Err("retained compiler artifact exceeds the supported limit".into());
    }
    Ok(bytes)
}
