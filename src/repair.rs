use crate::format::{HEADER_LEN, VERSION, corrupt, parse_header, r64, truncated};
use crate::varint::read_varint;
use std::io::{Read, Seek, SeekFrom, Write};

const MAX_SEGMENTS: u64 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepairOutcome {
    pub segments_kept: u64,

    pub dropped_bytes: u64,

    pub changed: bool,
}

pub fn repair(db_path: &str) -> std::io::Result<RepairOutcome> {
    let mut f = std::fs::File::open(db_path)?;
    let len = f.metadata()?.len();
    let (starts, pos, total) = walk(&mut f, len)?;

    if total > MAX_SEGMENTS {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "{total} segments exceed the {MAX_SEGMENTS}-segment reader cap: merge first, then repair"
            ),
        ));
    }
    if footer_matches(&mut f, len, pos, &starts)? {
        eprintln!(
            "repair: valid store ({} segments), no changes",
            starts.len()
        );
        return Ok(RepairOutcome {
            segments_kept: total,
            dropped_bytes: 0,
            changed: false,
        });
    }
    let dropped = len - pos;
    if dropped == 0 {
        if total == 0 {
            eprintln!("repair: empty file, no changes");
            return Ok(RepairOutcome {
                segments_kept: 0,
                dropped_bytes: 0,
                changed: false,
            });
        }
    } else if total == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "no valid segment found: refusing to truncate a foreign or hopelessly corrupt file; rebuild from sources",
        ));
    }
    drop(f);
    write_footer(db_path, &starts, pos, len)?;
    if dropped == 0 {
        eprintln!("repair: kept {} segments, footer appended", starts.len());
    } else {
        eprintln!(
            "repair: kept {} segments, dropped {} bytes, footer rewritten",
            starts.len(),
            dropped
        );
    }
    Ok(RepairOutcome {
        segments_kept: total,
        dropped_bytes: dropped,
        changed: true,
    })
}

fn walk(f: &mut std::fs::File, len: u64) -> std::io::Result<(Vec<u64>, u64, u64)> {
    let mut starts = Vec::new();
    let mut total = 0u64;
    let mut pos = 0u64;
    while pos < len {
        if len - pos < HEADER_LEN as u64 {
            break;
        }
        let mut hdr = [0u8; HEADER_LEN];
        f.seek(SeekFrom::Start(pos))?;
        f.read_exact(&mut hdr)?;
        let h = match parse_header(&hdr) {
            Ok(h) => h,

            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                if pos == 0 {
                    return Err(corrupt(
                        "bad magic at offset 0: not a .ulp store; rebuild from sources",
                    ));
                }
                break;
            }
            Err(e) => return Err(e),
        };
        if h.version != VERSION {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "segment at offset {pos} declares format version {}, expected {VERSION}: rebuild from sources",
                    h.version
                ),
            ));
        }

        let len_field = r64(&hdr, 120)
            .ok_or_else(|| truncated("segment length field out of bounds: corrupt .ulp"))?;
        let Some(end) = validate_segment(&h, len_field, f, len, pos)? else {
            break;
        };
        total += 1;
        if total > MAX_SEGMENTS {
            break;
        }
        starts.push(pos);
        pos = end;
    }
    Ok((starts, pos, total))
}

fn validate_segment(
    h: &crate::format::Header,
    len_field: u64,
    f: &mut std::fs::File,
    len: u64,
    pos: u64,
) -> std::io::Result<Option<u64>> {
    if h.url_dict_off != HEADER_LEN as u64 {
        return Ok(None);
    }
    if !((HEADER_LEN as u64) < h.user_dict_off
        && h.user_dict_off < h.pass_dict_off
        && h.pass_dict_off < h.records_off
        && h.records_off < h.user_idx_off
        && h.user_idx_off < h.pass_idx_off
        && h.pass_idx_off < h.misc_off)
    {
        return Ok(None);
    }
    let Some(rec_len) = records_section_len(h.record_count, h.url_count) else {
        return Ok(None);
    };
    if h.records_off.checked_add(rec_len) != Some(h.user_idx_off) {
        return Ok(None);
    }
    let Some(user_min) = section_min(h.user_idx_off, h.user_count) else {
        return Ok(None);
    };
    if user_min > h.pass_idx_off {
        return Ok(None);
    }
    let Some(pass_min) = section_min(h.pass_idx_off, h.pass_count) else {
        return Ok(None);
    };
    if pass_min > h.misc_off {
        return Ok(None);
    }

    let Some(misc_abs) = pos.checked_add(h.misc_off) else {
        return Ok(None);
    };
    if misc_abs.checked_add(8).is_none_or(|end| end > len) {
        return Ok(None);
    }
    f.seek(SeekFrom::Start(misc_abs))?;
    let mut count = [0u8; 8];
    f.read_exact(&mut count)?;
    if u64::from_le_bytes(count) != h.misc_count {
        return Ok(None);
    }
    let Some(entries_start) = misc_abs.checked_add(8) else {
        return Ok(None);
    };
    let derived_end = if h.misc_count == 0 {
        entries_start
    } else {
        let Some(end) = walk_misc(f, len, h.misc_count, entries_start)? else {
            return Ok(None);
        };
        end
    };

    if derived_end.checked_sub(pos) != Some(len_field) || derived_end > len {
        return Ok(None);
    }
    Ok(Some(derived_end))
}

fn section_min(off: u64, count: u64) -> Option<u64> {
    off.checked_add(8)?
        .checked_add(count.checked_add(1)?.checked_mul(4)?)
}

fn records_section_len(record_count: u64, url_count: u64) -> Option<u64> {
    let url_offsets = url_count.checked_add(1)?.checked_mul(4)?;
    let columns = record_count.checked_mul(8)?;
    let sep_bits = record_count.div_ceil(8).checked_mul(2)?;
    8u64.checked_add(url_offsets)?
        .checked_add(columns)?
        .checked_add(sep_bits)
}

fn walk_misc(
    f: &mut std::fs::File,
    len: u64,
    count: u64,
    start: u64,
) -> std::io::Result<Option<u64>> {
    let mut rpos = start;
    let mut reader = std::io::BufReader::with_capacity(64 << 10, f);
    for _ in 0..count {
        let mut buf = [0u8; 10];
        let mut n = 0usize;
        let entry_len = loop {
            if n == buf.len() || rpos >= len {
                return Ok(None);
            }
            reader.read_exact(&mut buf[n..n + 1])?;
            n += 1;
            rpos += 1;
            let mut p = 0usize;
            match read_varint(&buf[..n], &mut p) {
                Ok(v) => break v,
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {}
                Err(_) => return Ok(None),
            }
        };
        let Some(end) = rpos.checked_add(entry_len) else {
            return Ok(None);
        };
        if end > len {
            return Ok(None);
        }
        reader.seek(SeekFrom::Start(end))?;
        rpos = end;
    }
    Ok(Some(rpos))
}

fn footer_matches(
    f: &mut std::fs::File,
    len: u64,
    pos: u64,
    starts: &[u64],
) -> std::io::Result<bool> {
    if starts.is_empty() {
        return Ok(false);
    }
    let n = starts.len() as u64;
    let Some(footer_len) = n.checked_mul(8).and_then(|t| t.checked_add(16)) else {
        return Ok(false);
    };
    if len - pos != footer_len {
        return Ok(false);
    }
    let mut footer = vec![0u8; footer_len as usize];
    f.seek(SeekFrom::Start(pos))?;
    f.read_exact(&mut footer)?;
    if u64::from_le_bytes(
        footer[..8]
            .try_into()
            .map_err(|_| corrupt("footer too short: corrupt .ulp"))?,
    ) != n
    {
        return Ok(false);
    }
    if u64::from_le_bytes(
        footer[footer.len() - 8..]
            .try_into()
            .map_err(|_| corrupt("footer too short: corrupt .ulp"))?,
    ) != pos
    {
        return Ok(false);
    }
    for (i, &s) in starts.iter().enumerate() {
        let at = 8 + i * 8;
        if u64::from_le_bytes(
            footer[at..at + 8]
                .try_into()
                .map_err(|_| corrupt("footer offset table truncated: corrupt .ulp"))?,
        ) != s
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn write_footer(db_path: &str, starts: &[u64], cut: u64, walked_len: u64) -> std::io::Result<()> {
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(db_path)?;

    if f.metadata()?.len() != walked_len {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "the store changed during repair: re-run repair",
        ));
    }
    f.set_len(cut)?;
    f.seek(SeekFrom::Start(cut))?;
    let mut footer = Vec::with_capacity(16 + starts.len() * 8);
    footer.extend_from_slice(&(starts.len() as u64).to_le_bytes());
    for &s in starts {
        footer.extend_from_slice(&s.to_le_bytes());
    }
    footer.extend_from_slice(&cut.to_le_bytes());
    f.write_all(&footer)?;
    f.flush()?;
    f.sync_all()
}
