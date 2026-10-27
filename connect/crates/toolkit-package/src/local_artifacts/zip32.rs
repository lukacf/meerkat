//! Bounded metadata preflight for the v1 ZIP32 profile. This does not decompress
//! or implement CRC. zip 2.4.2 performs those operations through its streaming
//! reader, avoiding that version's allocating/deduplicating archive index.

use super::*;

struct Member {
    path: String,
    directory: bool,
    executable: bool,
    offset: u64,
    end: u64,
    size: u64,
    compressed_size: u64,
    crc: u32,
    method: u16,
    flags: u16,
    raw_name: Vec<u8>,
}

pub(super) fn materialize_zip(
    raw: &mut File,
    tree: Option<&File>,
    limits: &PreparationLimits,
) -> Result<Inventory> {
    scan_zip(raw, tree, limits, &mut BTreeMap::new(), 0, 0).map(|(inventory, _)| inventory)
}

pub(super) fn scan_zip(
    raw: &mut (impl Read + Seek),
    tree: Option<&File>,
    limits: &PreparationLimits,
    selected: &mut BTreeMap<String, Vec<u8>>,
    maximum_selected_file_bytes: u64,
    maximum_selected_bytes: u64,
) -> Result<(Inventory, usize)> {
    let members = preflight(raw, limits)?;
    let archive_entry_count = members.len();
    let mut selected_bytes = 0_u64;
    for path in selected.keys() {
        let member = members
            .iter()
            .find(|member| &member.path == path && !member.directory)
            .ok_or(PreparationError::Invalid("selected ZIP file is absent"))?;
        selected_bytes = selected_bytes
            .checked_add(member.size)
            .ok_or(PreparationError::Limit("selected ZIP bytes"))?;
        if member.size > maximum_selected_file_bytes || selected_bytes > maximum_selected_bytes {
            return Err(PreparationError::Limit("selected ZIP bytes"));
        }
    }
    let mut inventory = Inventory::new();
    let mut aliases = BTreeMap::new();
    for member in &members {
        let value = if member.directory {
            InventoryValue::Directory
        } else {
            InventoryValue::File {
                size: member.size,
                digest: ContentDigest::of_bytes(&[]),
                executable: member.executable,
            }
        };
        add_path(&mut inventory, &mut aliases, &member.path, value, limits)?;
    }
    if let Some(tree) = tree {
        create_tree_directories(tree, &inventory)?;
    }
    let mut remaining = limits.maximum_expanded_bytes;
    for member in members {
        raw.seek(SeekFrom::Start(member.offset))?;
        let mut bounded = (&mut *raw).take(member.end - member.offset);
        let mut decoded = zip::read::read_zipfile_from_stream(&mut bounded)?
            .ok_or(PreparationError::Invalid("missing local ZIP member"))?;
        if decoded.name_raw() != member.raw_name
            || decoded.size() != member.size
            || decoded.compressed_size() != member.compressed_size
            || decoded.crc32() != member.crc
            || decoded.encrypted()
        {
            return Err(PreparationError::Invalid(
                "ZIP decoder metadata differs from preflight",
            ));
        }
        let observed = if let Some(target) = selected.get_mut(&member.path) {
            // Inspection captures only explicitly selected, bounded files.
            // The same decoder and EOF/CRC checks still cover every member.
            hash_copy(&mut decoded, target, maximum_selected_file_bytes)?
        } else if !member.directory
            && let Some(tree) = tree
        {
            let (parent, name) = materialization_parent(tree, &member.path)?;
            let mut target = create_file(&parent, &name)?;
            let observed = hash_copy(
                &mut decoded,
                &mut target,
                remaining.min(limits.maximum_file_bytes),
            )?;
            seal_file(&target, member.executable)?;
            observed
        } else {
            hash_copy(
                &mut decoded,
                &mut io::sink(),
                remaining.min(limits.maximum_file_bytes),
            )?
        };
        // hash_copy reads through EOF, including the library's final CRC check.
        if observed.size != member.size {
            return Err(PreparationError::Invalid("ZIP expanded length differs"));
        }
        remaining -= observed.size;
        if !member.directory {
            inventory.insert(
                member.path,
                InventoryValue::File {
                    size: observed.size,
                    digest: observed.digest,
                    executable: member.executable,
                },
            );
        }
    }
    Ok((inventory, archive_entry_count))
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn preflight(raw: &mut (impl Read + Seek), limits: &PreparationLimits) -> Result<Vec<Member>> {
    let len = raw.seek(SeekFrom::End(0))?;
    if len > limits.maximum_raw_bytes {
        return Err(PreparationError::Limit("raw ZIP"));
    }
    if len < 22 {
        return Err(PreparationError::Invalid("missing ZIP end record"));
    }
    // ZIP32's end record plus maximum u16 comment. A unique candidate must end
    // exactly at EOF; we never ask zip to search for alternate end records.
    let tail_len = len.min(22 + u16::MAX as u64) as usize;
    raw.seek(SeekFrom::End(-(tail_len as i64)))?;
    let mut tail = vec![0_u8; tail_len];
    raw.read_exact(&mut tail)?;
    let candidates = (0..=tail_len - 22)
        .filter(|&index| {
            tail[index..index + 4] == *b"PK\x05\x06"
                && index + 22 + u16_at(&tail, index + 20) as usize == tail_len
        })
        .collect::<Vec<_>>();
    if candidates.len() != 1 {
        return Err(PreparationError::Invalid("ambiguous ZIP end record"));
    }
    let index = candidates[0];
    let end = &tail[index..index + 22];
    let end_offset = len - tail_len as u64 + index as u64;
    let count = u16_at(end, 10) as usize;
    let central_size = u32_at(end, 12) as u64;
    let central_start = u32_at(end, 16) as u64;
    if u16_at(end, 4) != 0 || u16_at(end, 6) != 0 || u16_at(end, 8) as usize != count {
        return Err(PreparationError::Unsupported("multi-disk ZIP"));
    }
    if count == u16::MAX as usize
        || central_size == u32::MAX as u64
        || central_start == u32::MAX as u64
    {
        return Err(PreparationError::Unsupported("ZIP64"));
    }
    if count == 0 || count > limits.maximum_entries {
        return Err(PreparationError::Limit("ZIP entries"));
    }
    if central_size > limits.maximum_archive_metadata_bytes
        || central_size + u16_at(end, 20) as u64 > limits.maximum_archive_metadata_bytes
    {
        return Err(PreparationError::Limit("ZIP metadata"));
    }
    if central_start.checked_add(central_size) != Some(end_offset) {
        return Err(PreparationError::Invalid("ZIP central directory range"));
    }
    // No unbounded allocation is performed from untrusted counts or name sizes.
    raw.seek(SeekFrom::Start(central_start))?;
    let mut remaining_metadata = central_size;
    let mut expanded = 0_u64;
    let mut members = Vec::new();
    let mut names = BTreeSet::new();
    for _ in 0..count {
        if remaining_metadata < 46 {
            return Err(PreparationError::Invalid("truncated ZIP directory"));
        }
        let mut header = [0_u8; 46];
        raw.read_exact(&mut header)?;
        if header[..4] != *b"PK\x01\x02" {
            return Err(PreparationError::Invalid("invalid ZIP central header"));
        }
        let name_len = u16_at(&header, 28) as usize;
        let extra_len = u16_at(&header, 30) as usize;
        let comment_len = u16_at(&header, 32) as usize;
        let record_len = 46 + name_len + extra_len + comment_len;
        if record_len as u64 > remaining_metadata {
            return Err(PreparationError::Invalid("ZIP metadata exceeds directory"));
        }
        remaining_metadata -= record_len as u64;
        if extra_len != 0 {
            return Err(PreparationError::Unsupported(
                "ZIP extra fields, including ZIP64",
            ));
        }
        if name_len == 0 || name_len > limits.maximum_path_bytes + 1 {
            return Err(PreparationError::Limit("ZIP path"));
        }
        let flags = u16_at(&header, 8);
        if flags & !0x0800 != 0 {
            return Err(PreparationError::Unsupported(
                "ZIP flags, encryption or data descriptors",
            ));
        }
        let method = u16_at(&header, 10);
        if !matches!(method, 0 | 8) {
            return Err(PreparationError::Unsupported("ZIP compression method"));
        }
        if u16_at(&header, 6) > 20 || u16_at(&header, 34) != 0 {
            return Err(PreparationError::Unsupported("ZIP version or disk"));
        }
        let size = u32_at(&header, 24) as u64;
        let compressed_size = u32_at(&header, 20) as u64;
        let offset = u32_at(&header, 42) as u64;
        if [size, compressed_size, offset].contains(&(u32::MAX as u64)) {
            return Err(PreparationError::Unsupported("ZIP64 member"));
        }
        expanded = expanded
            .checked_add(size)
            .ok_or(PreparationError::Limit("ZIP expanded bytes"))?;
        if size > limits.maximum_file_bytes || expanded > limits.maximum_expanded_bytes {
            return Err(PreparationError::Limit("ZIP expanded bytes"));
        }
        let mut raw_name = vec![0_u8; name_len];
        raw.read_exact(&mut raw_name)?;
        raw.seek(SeekFrom::Current(comment_len as i64))?;
        let name = std::str::from_utf8(&raw_name)
            .map_err(|_| PreparationError::Unsupported("non-ASCII ZIP path"))?;
        if !name.is_ascii() {
            return Err(PreparationError::Unsupported("non-ASCII ZIP path"));
        }
        if !names.insert(name.to_string()) {
            return Err(PreparationError::Invalid("duplicate ZIP member"));
        }
        let directory = name.ends_with('/');
        let path = if directory {
            &name[..name.len() - 1]
        } else {
            name
        };
        portable_path(path, limits.maximum_path_bytes)?;
        let creator = header[5];
        let external = u32_at(&header, 38);
        if !matches!(creator, 0 | 3) {
            return Err(PreparationError::Unsupported(
                "ZIP creator permission encoding",
            ));
        }
        let mode = if creator == 3 { external >> 16 } else { 0 };
        let file_type = mode & 0o170000;
        if (directory && !matches!(file_type, 0 | 0o040000))
            || (!directory && !matches!(file_type, 0 | 0o100000))
            || (!directory && external & 0x10 != 0)
        {
            return Err(PreparationError::Invalid(
                "ZIP symlink, special file or conflicting type",
            ));
        }
        if directory && (size != 0 || compressed_size != 0 || method != 0) {
            return Err(PreparationError::Invalid("ZIP directory carries payload"));
        }
        members.push(Member {
            path: path.to_string(),
            directory,
            executable: !directory && mode & 0o111 != 0,
            offset,
            end: 0,
            size,
            compressed_size,
            crc: u32_at(&header, 16),
            method,
            flags,
            raw_name,
        });
    }
    if remaining_metadata != 0 || raw.stream_position()? != end_offset {
        return Err(PreparationError::Invalid("extra ZIP central metadata"));
    }
    members.sort_by_key(|member| member.offset);
    let mut next_offset = 0_u64;
    for member in &mut members {
        if member.offset != next_offset {
            return Err(PreparationError::Invalid(
                "ZIP prefix, gap or overlapping members",
            ));
        }
        raw.seek(SeekFrom::Start(member.offset))?;
        let mut local = [0_u8; 30];
        raw.read_exact(&mut local)?;
        if local[..4] != *b"PK\x03\x04"
            || u16_at(&local, 4) > 20
            || u16_at(&local, 6) != member.flags
            || u16_at(&local, 8) != member.method
            || u32_at(&local, 14) != member.crc
            || u32_at(&local, 18) as u64 != member.compressed_size
            || u32_at(&local, 22) as u64 != member.size
            || u16_at(&local, 26) as usize != member.raw_name.len()
            || u16_at(&local, 28) != 0
        {
            return Err(PreparationError::Invalid(
                "ZIP local/central metadata disagree",
            ));
        }
        let mut name = vec![0_u8; member.raw_name.len()];
        raw.read_exact(&mut name)?;
        if name != member.raw_name {
            return Err(PreparationError::Invalid(
                "ZIP local/central names disagree",
            ));
        }
        member.end = raw
            .stream_position()?
            .checked_add(member.compressed_size)
            .ok_or(PreparationError::Invalid("ZIP member range overflow"))?;
        if member.end > central_start {
            return Err(PreparationError::Invalid("ZIP member overlaps directory"));
        }
        next_offset = member.end;
    }
    if next_offset != central_start {
        return Err(PreparationError::Invalid(
            "ZIP trailing gap or hidden member",
        ));
    }
    Ok(members)
}
