//! Reads the file list of a disc image (.iso/.img/.udf) without mounting it.
//!
//! DVD and Blu-ray images carry a UDF file system (Blu-ray uses UDF 2.50, whose directories live
//! in a "metadata partition"); some DVD masters only have ISO 9660. Only what is needed to list
//! files and read small ones is implemented: no writing, no rewritable-media features.

use std::collections::HashMap;
use std::fs::File;
use std::io;

const SECTOR: u64 = 2048;

/// A run of a file's bytes.
#[derive(Clone, Debug)]
pub enum Piece {
    /// `len` bytes at byte `offset` in the image.
    Data { offset: u64, len: u64 },
    /// Allocated but unrecorded space, read as zeros.
    Zero { len: u64 },
    /// Data stored inside the file entry itself.
    Inline(Vec<u8>),
}

#[derive(Clone, Debug)]
pub struct Entry {
    /// '/'-separated, relative to the volume root.
    pub path: String,
    pub size: u64,
    pub pieces: Vec<Piece>,
}

pub struct Volume {
    pub label: String,
    pub files: Vec<Entry>,
}

/// Reads the image's file system: UDF when present, otherwise ISO 9660.
pub fn read_volume(file: &File) -> io::Result<Volume> {
    if is_udf(file)? {
        return Udf::open(file)?.volume();
    }

    if let Some(v) = iso9660::read(file)? {
        return Ok(v);
    }

    Err(invalid("no UDF or ISO 9660 file system found"))
}

/// Reads the whole content of an entry (for small files: IFO, MPLS).
pub fn read_entry(file: &File, entry: &Entry) -> io::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(entry.size as usize);
    for piece in &entry.pieces {
        match piece {
            Piece::Data { offset, len } => {
                let start = out.len();
                out.resize(start + *len as usize, 0);
                read_at(file, *offset, &mut out[start..])?;
            }
            Piece::Zero { len } => out.resize(out.len() + *len as usize, 0),
            Piece::Inline(bytes) => out.extend_from_slice(bytes),
        }
    }

    out.truncate(entry.size as usize);
    Ok(out)
}

pub fn read_at(file: &File, offset: u64, buf: &mut [u8]) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut done = 0;
        while done < buf.len() {
            let n = file.seek_read(&mut buf[done..], offset + done as u64)?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "read past the end of the image",
                ));
            }
            done += n;
        }
        Ok(())
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.read_exact_at(buf, offset)
    }
}

fn read_sector(file: &File, sector: u64) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; SECTOR as usize];
    read_at(file, sector * SECTOR, &mut buf)?;
    Ok(buf)
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

fn u16le(d: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([d[at], d[at + 1]])
}

fn u32le(d: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]])
}

fn u64le(d: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(d[at..at + 8].try_into().unwrap())
}

/// UDF has an anchor volume descriptor pointer (tag 2) at sector 256.
fn is_udf(file: &File) -> io::Result<bool> {
    let len = file.metadata()?.len();
    if len < 257 * SECTOR {
        return Ok(false);
    }

    let s = read_sector(file, 256)?;
    Ok(u16le(&s, 0) == 2 && u32le(&s, 12) == 256)
}

// ------------------------------------------------------------------------------------- UDF

#[derive(Clone, Copy)]
enum PartitionMap {
    /// Blocks map straight onto the partition with this number.
    Physical(u16),
    /// UDF 2.50 metadata partition: blocks map through the metadata file's extents.
    Metadata { partition: u16, file_location: u32 },
}

struct Udf<'a> {
    file: &'a File,
    block: u64,
    starts: HashMap<u16, u64>,
    maps: Vec<PartitionMap>,
    /// Extents of the metadata file (physical partition blocks), for each metadata map.
    metadata: HashMap<usize, Vec<(u64, u64)>>,
    label: String,
    fileset: (u16, u32),
}

/// One allocation descriptor, resolved to partition-relative blocks.
struct Alloc {
    kind: u32,
    len: u64,
    partition: u16,
    block: u32,
}

impl<'a> Udf<'a> {
    fn open(file: &'a File) -> io::Result<Self> {
        let anchor = read_sector(file, 256)?;
        let vds_len = u32le(&anchor, 16) as u64;
        let vds_start = u32le(&anchor, 20) as u64;
        let mut udf = Udf {
            file,
            block: SECTOR,
            starts: HashMap::new(),
            maps: Vec::new(),
            metadata: HashMap::new(),
            label: String::new(),
            fileset: (0, 0),
        };
        let mut pvd_label = String::new();
        let mut have_lvd = false;
        for i in 0..(vds_len / SECTOR).max(1) {
            let d = read_sector(file, vds_start + i)?;
            match u16le(&d, 0) {
                1 => pvd_label = dstring(&d[24..56]),
                5 => {
                    udf.starts.insert(u16le(&d, 22), u32le(&d, 188) as u64);
                }
                6 => {
                    have_lvd = true;
                    udf.label = dstring(&d[84..212]);
                    udf.block = u32le(&d, 212) as u64;
                    udf.fileset = (u16le(&d, 248 + 8), u32le(&d, 248 + 4));
                    let count = u32le(&d, 268) as usize;
                    let mut at = 440;
                    for _ in 0..count {
                        if at + 2 > d.len() {
                            break;
                        }
                        let (kind, len) = (d[at], d[at + 1] as usize);
                        if kind == 1 {
                            udf.maps.push(PartitionMap::Physical(u16le(&d, at + 4)));
                        } else if kind == 2 {
                            let ident = &d[at + 5..at + 28];
                            let partition = u16le(&d, at + 38);
                            if ident.starts_with(b"*UDF Metadata Partition") {
                                udf.maps.push(PartitionMap::Metadata {
                                    partition,
                                    file_location: u32le(&d, at + 40),
                                });
                            } else if ident.starts_with(b"*UDF Virtual Partition") {
                                return Err(invalid(
                                    "UDF virtual partitions (written CD/DVD-R sessions) are not supported",
                                ));
                            } else {
                                // Sparable partitions only remap defects on rewritable media.
                                udf.maps.push(PartitionMap::Physical(partition));
                            }
                        }
                        at += len.max(6);
                    }
                }
                8 => break,
                _ => {}
            }
        }

        if !have_lvd || udf.maps.is_empty() {
            return Err(invalid("UDF logical volume descriptor not found"));
        }

        if udf.label.trim().is_empty() {
            udf.label = pvd_label;
        }

        for i in 0..udf.maps.len() {
            if let PartitionMap::Metadata {
                partition,
                file_location,
            } = udf.maps[i]
            {
                let entry = udf.read_physical_block(partition, file_location)?;
                let allocs = udf.allocations(&entry, 0, Some(partition))?;
                let extents = allocs.iter().map(|a| (a.block as u64, a.len)).collect();
                udf.metadata.insert(i, extents);
            }
        }

        Ok(udf)
    }

    fn volume(&self) -> io::Result<Volume> {
        let fsd = self.read_block(self.fileset.0, self.fileset.1)?;
        if u16le(&fsd, 0) != 256 {
            return Err(invalid("UDF file set descriptor not found"));
        }

        let root = (u16le(&fsd, 400 + 8), u32le(&fsd, 400 + 4));
        let mut files = Vec::new();
        self.walk(root, "", 0, &mut files)?;
        Ok(Volume {
            label: self.label.trim().to_string(),
            files,
        })
    }

    fn walk(
        &self,
        icb: (u16, u32),
        prefix: &str,
        depth: usize,
        out: &mut Vec<Entry>,
    ) -> io::Result<()> {
        if depth > 32 {
            return Err(invalid("UDF directory tree is too deep"));
        }

        let entry = self.read_block(icb.0, icb.1)?;
        let dir = self.content(&entry, icb.0)?;
        let mut at = 0;
        while at + 38 <= dir.len() {
            if u16le(&dir, at) != 257 {
                break;
            }

            let flags = dir[at + 18];
            let name_len = dir[at + 19] as usize;
            let child = (u16le(&dir, at + 20 + 8), u32le(&dir, at + 20 + 4));
            let iu_len = u16le(&dir, at + 36) as usize;
            let name_at = at + 38 + iu_len;
            let total = (38 + iu_len + name_len).div_ceil(4) * 4;
            let parent_or_deleted = flags & 0x08 != 0 || flags & 0x04 != 0;
            if !parent_or_deleted && name_at + name_len <= dir.len() {
                let name = dstring_raw(&dir[name_at..name_at + name_len]);
                let path = if prefix.is_empty() {
                    name
                } else {
                    format!("{prefix}/{name}")
                };
                if flags & 0x02 != 0 {
                    self.walk(child, &path, depth + 1, out)?;
                } else {
                    let fe = self.read_block(child.0, child.1)?;
                    let size = info_length(&fe)?;
                    let pieces = self.pieces(&fe, child.0, size)?;
                    out.push(Entry { path, size, pieces });
                }
            }

            at += total;
        }

        Ok(())
    }

    /// The whole content of a (small) file entry, e.g. a directory.
    fn content(&self, fe: &[u8], partition: u16) -> io::Result<Vec<u8>> {
        let size = info_length(fe)?;
        if size > 64 * 1024 * 1024 {
            return Err(invalid("UDF directory is unreasonably large"));
        }

        let pieces = self.pieces(fe, partition, size)?;
        read_entry(
            self.file,
            &Entry {
                path: String::new(),
                size,
                pieces,
            },
        )
    }

    fn pieces(&self, fe: &[u8], partition: u16, size: u64) -> io::Result<Vec<Piece>> {
        let (ad_start, ad_len) = ad_area(fe)?;
        if icb_flags(fe) & 7 == 3 {
            let end = (ad_start + ad_len).min(fe.len());
            return Ok(vec![Piece::Inline(fe[ad_start..end].to_vec())]);
        }

        let mut pieces = Vec::new();
        let mut remaining = size;
        for a in self.allocations(fe, partition, None)? {
            if remaining == 0 {
                break;
            }
            let len = a.len.min(remaining);
            remaining -= len;
            if a.kind == 0 {
                pieces.push(Piece::Data {
                    offset: self.offset(a.partition, a.block)?,
                    len,
                });
            } else {
                pieces.push(Piece::Zero { len });
            }
        }

        Ok(pieces)
    }

    /// Allocation descriptors of a file entry, following continuation extents. Short descriptors
    /// are relative to partition reference `reference`; for the metadata file itself, `physical`
    /// names the physical partition (by number) its descriptors point into.
    fn allocations(
        &self,
        fe: &[u8],
        reference: u16,
        physical: Option<u16>,
    ) -> io::Result<Vec<Alloc>> {
        let kind = icb_flags(fe) & 7;
        let (start, len) = ad_area(fe)?;
        let mut out = Vec::new();
        let mut area = fe[start..(start + len).min(fe.len())].to_vec();
        let mut hops = 0;
        loop {
            let size = match kind {
                0 => 8,
                1 => 16,
                2 => 20,
                _ => return Ok(out),
            };
            let mut next = None;
            let mut at = 0;
            while at + size <= area.len() {
                let raw = u32le(&area, at);
                let ext_kind = raw >> 30;
                let ext_len = (raw & 0x3FFF_FFFF) as u64;
                if ext_len == 0 {
                    break;
                }
                let (block, part) = match kind {
                    0 => (u32le(&area, at + 4), reference),
                    1 => (u32le(&area, at + 4), u16le(&area, at + 8)),
                    _ => (u32le(&area, at + 12), u16le(&area, at + 16)),
                };
                if ext_kind == 3 {
                    next = Some((part, block));
                    break;
                }
                out.push(Alloc {
                    kind: ext_kind,
                    len: ext_len,
                    partition: part,
                    block,
                });
                at += size;
            }

            let Some((part, block)) = next else {
                return Ok(out);
            };
            hops += 1;
            if hops > 1024 {
                return Err(invalid("UDF allocation extent chain is too long"));
            }
            let aed = match physical {
                Some(number) => self.read_physical_block(number, block)?,
                None => self.read_block(part, block)?,
            };
            let l_ad = u32le(&aed, 20) as usize;
            area = aed[24..(24 + l_ad).min(aed.len())].to_vec();
        }
    }

    /// Byte offset in the image of a block in a partition (by partition map reference).
    fn offset(&self, reference: u16, block: u32) -> io::Result<u64> {
        let map = *self
            .maps
            .get(reference as usize)
            .ok_or_else(|| invalid("UDF partition reference out of range"))?;
        match map {
            PartitionMap::Physical(number) => self.physical_offset(number, block as u64),
            PartitionMap::Metadata { partition, .. } => {
                let extents = &self.metadata[&(reference as usize)];
                let mut byte = block as u64 * self.block;
                for &(start, len) in extents {
                    if byte < len {
                        return self.physical_offset(partition, start).map(|o| o + byte);
                    }
                    byte -= len;
                }
                Err(invalid("UDF metadata block out of range"))
            }
        }
    }

    fn physical_offset(&self, partition: u16, block: u64) -> io::Result<u64> {
        let start = self
            .starts
            .get(&partition)
            .ok_or_else(|| invalid("UDF partition descriptor missing"))?;
        Ok((start + block) * self.block)
    }

    fn read_block(&self, reference: u16, block: u32) -> io::Result<Vec<u8>> {
        let mut buf = vec![0u8; self.block as usize];
        read_at(self.file, self.offset(reference, block)?, &mut buf)?;
        Ok(buf)
    }

    fn read_physical_block(&self, partition: u16, block: u32) -> io::Result<Vec<u8>> {
        let mut buf = vec![0u8; self.block as usize];
        read_at(
            self.file,
            self.physical_offset(partition, block as u64)?,
            &mut buf,
        )?;
        Ok(buf)
    }
}

fn icb_flags(fe: &[u8]) -> u16 {
    u16le(fe, 16 + 18)
}

fn info_length(fe: &[u8]) -> io::Result<u64> {
    match u16le(fe, 0) {
        261 | 266 => Ok(u64le(fe, 56)),
        _ => Err(invalid("UDF file entry not found where expected")),
    }
}

/// Start and length of the allocation descriptors in a file entry or extended file entry.
fn ad_area(fe: &[u8]) -> io::Result<(usize, usize)> {
    match u16le(fe, 0) {
        261 => Ok((176 + u32le(fe, 168) as usize, u32le(fe, 172) as usize)),
        266 => Ok((216 + u32le(fe, 208) as usize, u32le(fe, 212) as usize)),
        _ => Err(invalid("UDF file entry not found where expected")),
    }
}

/// A fixed-size "dstring": the last byte holds the used length.
fn dstring(field: &[u8]) -> String {
    let used = *field.last().unwrap_or(&0) as usize;
    if used == 0 || used > field.len() - 1 {
        return String::new();
    }
    dstring_raw(&field[..used])
}

/// OSTA compressed unicode: a compression id (8 = one byte per char, 16 = UTF-16BE), then the text.
fn dstring_raw(bytes: &[u8]) -> String {
    match bytes.first() {
        Some(8) => bytes[1..].iter().map(|&b| b as char).collect(),
        Some(16) => {
            let units: Vec<u16> = bytes[1..]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        }
        _ => String::new(),
    }
}

// -------------------------------------------------------------------------------- ISO 9660

mod iso9660 {
    use super::*;

    pub fn read(file: &File) -> io::Result<Option<Volume>> {
        let mut primary = None;
        let mut joliet = None;
        for sector in 16..64u64 {
            let d = match read_sector(file, sector) {
                Ok(d) => d,
                Err(_) => break,
            };
            if &d[1..6] != b"CD001" {
                break;
            }
            match d[0] {
                1 if primary.is_none() => primary = Some(d),
                2 if matches!(&d[88..91], b"%/@" | b"%/C" | b"%/E") => joliet = Some(d),
                255 => break,
                _ => {}
            }
        }

        let (descriptor, wide) = match (joliet, primary) {
            (Some(j), _) => (j, true),
            (None, Some(p)) => (p, false),
            _ => return Ok(None),
        };

        let block = u16le(&descriptor, 128) as u64;
        let label = name(&descriptor[40..72], wide).trim().to_string();
        let root = &descriptor[156..190];
        let mut files: Vec<Entry> = Vec::new();
        walk(
            file,
            block,
            u32le(root, 2) as u64,
            u32le(root, 10) as u64,
            "",
            wide,
            0,
            &mut files,
        )?;
        Ok(Some(Volume { label, files }))
    }

    #[allow(clippy::too_many_arguments)]
    fn walk(
        file: &File,
        block: u64,
        extent: u64,
        size: u64,
        prefix: &str,
        wide: bool,
        depth: usize,
        out: &mut Vec<Entry>,
    ) -> io::Result<()> {
        if depth > 32 || size > 64 * 1024 * 1024 {
            return Err(invalid("ISO 9660 directory tree is malformed"));
        }

        let mut data = vec![0u8; size as usize];
        read_at(file, extent * block, &mut data)?;
        let mut at = 0usize;
        let mut pending = false;
        while at < data.len() {
            let len = data[at] as usize;
            if len == 0 {
                at = (at / block as usize + 1) * block as usize; // records never cross a sector
                continue;
            }
            if at + 33 > data.len() || at + len > data.len() {
                break;
            }

            let r = &data[at..at + len];
            let flags = r[25];
            let name_len = r[32] as usize;
            let raw = &r[33..33 + name_len.min(len - 33)];
            at += len;
            if raw == [0] || raw == [1] {
                continue;
            }

            let mut n = name(raw, wide);
            if let Some(i) = n.rfind(';') {
                n.truncate(i);
            }
            if n.ends_with('.') {
                n.pop();
            }

            let path = if prefix.is_empty() {
                n
            } else {
                format!("{prefix}/{n}")
            };
            let location = u32le(r, 2) as u64;
            let length = u32le(r, 10) as u64;
            if flags & 0x02 != 0 {
                walk(file, block, location, length, &path, wide, depth + 1, out)?;
                continue;
            }

            let piece = Piece::Data {
                offset: location * block,
                len: length,
            };
            // A file over 4 GB is split into several records with the same name; all but the
            // last have the multi-extent flag set.
            match out.last_mut() {
                Some(last) if pending && last.path == path => {
                    last.size += length;
                    last.pieces.push(piece);
                }
                _ => out.push(Entry {
                    path,
                    size: length,
                    pieces: vec![piece],
                }),
            }
            pending = flags & 0x80 != 0;
        }

        Ok(())
    }

    fn name(bytes: &[u8], wide: bool) -> String {
        if wide {
            let units: Vec<u16> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        } else {
            bytes.iter().map(|&b| b as char).collect()
        }
    }
}
