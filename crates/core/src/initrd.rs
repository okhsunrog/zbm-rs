//! Bounded inspection of initramfs /init, without extracting or executing it.
use std::{
    fs,
    io::{self, Read},
    path::Path,
};
const LIMIT: u64 = 128 * 1024 * 1024;
fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Unsupported or malformed initrd archive",
    )
}
fn hex(bytes: &[u8]) -> io::Result<usize> {
    usize::from_str_radix(std::str::from_utf8(bytes).map_err(|_| invalid())?, 16)
        .map_err(|_| invalid())
}
fn align(value: usize) -> io::Result<usize> {
    value.checked_add(3).map(|v| v & !3).ok_or_else(invalid)
}
fn bounded(reader: impl Read) -> io::Result<Vec<u8>> {
    let mut bytes = vec![];
    reader.take(LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > LIMIT {
        return Err(invalid());
    }
    Ok(bytes)
}
pub fn systemd(path: &Path) -> io::Result<bool> {
    let mut bytes = bounded(fs::File::open(path)?)?;
    let mut result = false;
    // Early microcode may precede the compressed main cpio. Last /init wins.
    for _ in 0..16 {
        let mut position: usize = 0;
        if bytes.starts_with(b"\x1f\x8b") {
            bytes = bounded(flate2::read::MultiGzDecoder::new(bytes.as_slice()))?;
        } else if bytes.starts_with(b"\x28\xb5\x2f\xfd") {
            bytes = bounded(zstd::stream::read::Decoder::new(bytes.as_slice())?)?;
        } else if bytes.starts_with(b"\xfd7zXZ\0") {
            let mut output = Limited { bytes: vec![] };
            lzma_rs::xz_decompress(&mut std::io::Cursor::new(&bytes), &mut output)
                .map_err(|_| invalid())?;
            bytes = output.bytes;
        }
        loop {
            let header = bytes
                .get(position..position.checked_add(110).ok_or_else(invalid)?)
                .ok_or_else(invalid)?;
            if !header.starts_with(b"070701") && !header.starts_with(b"070702") {
                return Err(invalid());
            }
            let size = hex(&header[54..62])?;
            let namesize = hex(&header[94..102])?;
            let name_end = position
                .checked_add(110)
                .and_then(|p| p.checked_add(namesize))
                .ok_or_else(invalid)?;
            let name = bytes.get(position + 110..name_end).ok_or_else(invalid)?;
            let name = name.strip_suffix(&[0]).ok_or_else(invalid)?;
            let data_start = align(name_end)?;
            let data_end = data_start.checked_add(size).ok_or_else(invalid)?;
            let data = bytes.get(data_start..data_end).ok_or_else(invalid)?;
            if name == b"init" || name == b"./init" {
                let mode = hex(&header[14..22])?;
                result = mode & 0xf000 == 0xa000
                    && (data.ends_with(b"/lib/systemd/systemd") || data.ends_with(b"/bin/systemd"));
            }
            position = align(data_end)?;
            if name == b"TRAILER!!!" {
                break;
            }
        }
        let remainder = bytes.get(position..).ok_or_else(invalid)?;
        let Some(next) = remainder.iter().position(|b| *b != 0) else {
            return Ok(result);
        };
        bytes = remainder[next..].to_vec();
    }
    Err(invalid())
}
struct Limited {
    bytes: Vec<u8>,
}
impl io::Write for Limited {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.bytes.len() as u64 + bytes.len() as u64 > LIMIT {
            return Err(invalid());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn archive(init: &str) -> Vec<u8> {
        let mut bytes = vec![];
        for (name, data, mode) in [
            ("init", init.as_bytes(), 0xa1ffu32),
            ("TRAILER!!!", &[][..], 0),
        ] {
            bytes.extend_from_slice(b"070701");
            for value in [
                1,
                mode,
                0,
                0,
                1,
                0,
                data.len() as u32,
                0,
                0,
                0,
                0,
                name.len() as u32 + 1,
                0,
            ] {
                bytes.extend_from_slice(format!("{value:08x}").as_bytes());
            }
            bytes.extend_from_slice(name.as_bytes());
            bytes.push(0);
            while bytes.len() % 4 != 0 {
                bytes.push(0);
            }
            bytes.extend_from_slice(data);
            while bytes.len() % 4 != 0 {
                bytes.push(0);
            }
        }
        bytes
    }
    #[test]
    fn recognizes_actual_init_and_rejects_malformed_archives() {
        let path = std::env::temp_dir().join(format!("zbm-initrd-{}", std::process::id()));
        fs::write(&path, archive("/usr/lib/systemd/systemd")).unwrap();
        assert!(systemd(&path).unwrap());
        fs::write(&path, archive("/bin/busybox")).unwrap();
        assert!(!systemd(&path).unwrap());
        let early = archive("/bin/busybox");
        let compressed = zstd::stream::encode_all(
            archive("/nix/store/hash-systemd/lib/systemd/systemd").as_slice(),
            1,
        )
        .unwrap();
        fs::write(&path, [early, compressed].concat()).unwrap();
        assert!(systemd(&path).unwrap());
        fs::write(&path, b"not an initrd").unwrap();
        assert!(systemd(&path).is_err());
        fs::remove_file(path).unwrap();
    }
}
