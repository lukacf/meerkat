#![allow(clippy::expect_used)] // Test utility — panics are acceptable

use flate2::Compression;
use flate2::write::GzEncoder;
use tar::EntryType;

pub fn build_custom_archive(path: &str, entry_type: EntryType, body: &[u8]) -> Vec<u8> {
    let typeflag = if entry_type.is_symlink() {
        b'2'
    } else if entry_type.is_hard_link() {
        b'1'
    } else {
        b'0'
    };

    let mut header = [0u8; 512];
    let path_bytes = path.as_bytes();
    let name_len = path_bytes.len().min(100);
    header[..name_len].copy_from_slice(&path_bytes[..name_len]);
    write_octal(&mut header[100..108], 0o644);
    write_octal(&mut header[108..116], 0);
    write_octal(&mut header[116..124], 0);
    write_octal(&mut header[124..136], body.len() as u64);
    write_octal(&mut header[136..148], 0);
    header[148..156].fill(b' ');
    header[156] = typeflag;
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let checksum = header.iter().map(|byte| u32::from(*byte)).sum::<u32>();
    write_checksum(&mut header[148..156], checksum);

    let mut tar_bytes = Vec::new();
    tar_bytes.extend_from_slice(&header);
    tar_bytes.extend_from_slice(body);
    let pad_len = (512 - (body.len() % 512)) % 512;
    if pad_len > 0 {
        tar_bytes.extend(std::iter::repeat_n(0u8, pad_len));
    }
    tar_bytes.extend(std::iter::repeat_n(0u8, 1024));

    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    std::io::Write::write_all(&mut encoder, &tar_bytes).expect("write gz");
    encoder.finish().expect("finish gz")
}

fn write_octal(dst: &mut [u8], value: u64) {
    let width = dst.len();
    let mut encoded = format!("{value:o}");
    if encoded.len() + 1 > width {
        encoded = "0".repeat(width - 1);
    }
    let padded = format!("{encoded:0>width$}", width = width - 1);
    dst[..width - 1].copy_from_slice(padded.as_bytes());
    dst[width - 1] = 0;
}

fn write_checksum(dst: &mut [u8], checksum: u32) {
    let width = dst.len();
    let mut encoded = format!("{checksum:o}");
    if encoded.len() + 2 > width {
        encoded = "0".repeat(width - 2);
    }
    let padded = format!("{encoded:0>width$}", width = width - 2);
    dst[..width - 2].copy_from_slice(padded.as_bytes());
    dst[width - 2] = 0;
    dst[width - 1] = b' ';
}
