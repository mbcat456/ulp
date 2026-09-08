use crate::format::{
    Header, corrupt, parse_header, posting_section_min_len_u64, r32, records_section_len_u64,
    truncated,
};
use crate::frontcode;

pub(crate) struct Reader<'a> {
    s: &'a [u8],
    pub(crate) hdr: Header,
    pub(crate) url_dict: frontcode::Dict<'a>,
    pub(crate) user_dict: frontcode::Dict<'a>,
    pub(crate) pass_dict: frontcode::Dict<'a>,
    pub(crate) url_offsets_base: u64,
    pub(crate) user_col_base: u64,
    pub(crate) pass_col_base: u64,
    pub(crate) sep_base: u64,
    pub(crate) sep2_base: u64,
    pub(crate) user_off_base: u64,
    pub(crate) user_blob_base: u64,
    pub(crate) pass_off_base: u64,
    pub(crate) pass_blob_base: u64,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(s: &'a [u8]) -> std::io::Result<Reader<'a>> {
        let hdr = parse_header(s)?;
        let offs = [
            hdr.url_dict_off,
            hdr.user_dict_off,
            hdr.pass_dict_off,
            hdr.records_off,
            hdr.user_idx_off,
            hdr.pass_idx_off,
            hdr.misc_off,
        ];
        if hdr.url_dict_off != crate::format::HEADER_LEN as u64
            || offs.windows(2).any(|w| w[0] >= w[1])
            || hdr.misc_off > s.len() as u64
        {
            return Err(corrupt("header section offsets out of order: corrupt .ulp"));
        }
        let expected_records = records_section_len_u64(hdr.record_count, hdr.url_count)
            .ok_or_else(|| corrupt("record section length overflows: corrupt .ulp"))?;
        if hdr.records_off.checked_add(expected_records) != Some(hdr.user_idx_off) {
            return Err(corrupt("record section length mismatch: corrupt .ulp"));
        }
        let user_section_len = hdr.pass_idx_off - hdr.user_idx_off;
        let pass_section_len = hdr.misc_off - hdr.pass_idx_off;
        if posting_section_min_len_u64(hdr.user_count).is_none_or(|min| min > user_section_len)
            || posting_section_min_len_u64(hdr.pass_count).is_none_or(|min| min > pass_section_len)
        {
            return Err(corrupt(
                "posting section is too short for its key count: corrupt .ulp",
            ));
        }

        let as_usize = |v: u64| {
            usize::try_from(v)
                .map_err(|_| corrupt("segment offset exceeds address space: corrupt .ulp"))
        };
        let fit = |v: u64| {
            usize::try_from(v)
                .map(|_| v)
                .map_err(|_| corrupt("segment offset exceeds address space: corrupt .ulp"))
        };
        let url_dict_start = as_usize(hdr.url_dict_off)?;
        let url_dict_end = as_usize(hdr.user_dict_off)?;
        let user_dict_start = as_usize(hdr.user_dict_off)?;
        let user_dict_end = as_usize(hdr.pass_dict_off)?;
        let pass_dict_start = as_usize(hdr.pass_dict_off)?;
        let pass_dict_end = as_usize(hdr.records_off)?;
        let url_dict = frontcode::Dict::parse(
            s.get(url_dict_start..url_dict_end)
                .ok_or_else(|| truncated("url dict offset beyond file: corrupt .ulp"))?,
        )?;
        let user_dict = frontcode::Dict::parse(
            s.get(user_dict_start..user_dict_end)
                .ok_or_else(|| truncated("user dict offset beyond file: corrupt .ulp"))?,
        )?;
        let pass_dict = frontcode::Dict::parse(
            s.get(pass_dict_start..pass_dict_end)
                .ok_or_else(|| truncated("pass dict offset beyond file: corrupt .ulp"))?,
        )?;
        let add = |base: u64, len: u64| {
            base.checked_add(len)
                .ok_or_else(|| corrupt("segment offset overflows: corrupt .ulp"))
        };
        let mul4 = |v: u64| {
            v.checked_mul(4)
                .ok_or_else(|| corrupt("segment offset overflows: corrupt .ulp"))
        };
        let url_offsets_base = add(hdr.records_off, 8)?;
        let user_col_base = add(
            url_offsets_base,
            mul4(
                hdr.url_count
                    .checked_add(1)
                    .ok_or_else(|| corrupt("url count overflows: corrupt .ulp"))?,
            )?,
        )?;
        let pass_col_base = add(user_col_base, mul4(hdr.record_count)?)?;
        let sep_base = add(pass_col_base, mul4(hdr.record_count)?)?;
        let sep2_base = add(sep_base, hdr.record_count.div_ceil(8))?;
        let user_off_base = add(hdr.user_idx_off, 8)?;
        let user_blob_base = add(
            user_off_base,
            mul4(
                hdr.user_count
                    .checked_add(1)
                    .ok_or_else(|| corrupt("user count overflows: corrupt .ulp"))?,
            )?,
        )?;
        let pass_off_base = add(hdr.pass_idx_off, 8)?;
        let pass_blob_base = add(
            pass_off_base,
            mul4(
                hdr.pass_count
                    .checked_add(1)
                    .ok_or_else(|| corrupt("pass count overflows: corrupt .ulp"))?,
            )?,
        )?;
        Ok(Reader {
            s,
            hdr,
            url_dict,
            user_dict,
            pass_dict,
            url_offsets_base: fit(url_offsets_base)?,
            user_col_base: fit(user_col_base)?,
            pass_col_base: fit(pass_col_base)?,
            sep_base: fit(sep_base)?,
            sep2_base: fit(sep2_base)?,
            user_off_base: fit(user_off_base)?,
            user_blob_base: fit(user_blob_base)?,
            pass_off_base: fit(pass_off_base)?,
            pass_blob_base: fit(pass_blob_base)?,
        })
    }

    #[inline]
    pub(crate) fn url_id_of(&self, idx: u64) -> std::io::Result<u32> {
        let n = (self.hdr.url_count as usize)
            .checked_add(1)
            .ok_or_else(|| corrupt("url count overflows address space: corrupt .ulp"))?;
        let base = self.url_offsets_base as usize;

        let mut lo = 0usize;
        let mut hi = n;
        while lo < hi {
            let mid = (lo + hi) / 2;
            let o = base
                .checked_add(
                    mid.checked_mul(4)
                        .ok_or_else(|| corrupt("url offset table index overflows: corrupt .ulp"))?,
                )
                .ok_or_else(|| corrupt("url offset table offset overflows: corrupt .ulp"))?;
            if r32(self.s, o)
                .ok_or_else(|| truncated("url offset table out of range: corrupt .ulp"))?
                as u64
                <= idx
            {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo == 0 {
            return Err(corrupt("url offset table empty: corrupt .ulp"));
        }
        Ok((lo - 1) as u32)
    }

    #[inline]
    fn sep_of(&self, base: u64, idx: u64) -> std::io::Result<u8> {
        let off = base
            .checked_add(idx / 8)
            .ok_or_else(|| corrupt("separator bit offset overflows: corrupt .ulp"))?;
        let b = *self
            .s
            .get(off as usize)
            .ok_or_else(|| truncated("separator bit out of range: corrupt .ulp"))?;
        Ok(if (b >> (idx % 8)) & 1 == 1 {
            b'|'
        } else {
            b':'
        })
    }

    #[inline]
    fn col_offset(&self, base: u64, idx: u64) -> std::io::Result<usize> {
        base.checked_add(
            idx.checked_mul(4)
                .ok_or_else(|| corrupt("record column index overflows: corrupt .ulp"))?,
        )
        .ok_or_else(|| corrupt("record column offset overflows: corrupt .ulp"))
        .map(|v| v as usize)
    }

    #[inline]
    pub(crate) fn output_record(&self, out: &mut Vec<u8>, idx: u64) -> std::io::Result<()> {
        let url_id = self.url_id_of(idx)?;
        let user_id = r32(self.s, self.col_offset(self.user_col_base, idx)?)
            .ok_or_else(|| truncated("user column out of range: corrupt .ulp"))?;
        let pass_id = r32(self.s, self.col_offset(self.pass_col_base, idx)?)
            .ok_or_else(|| truncated("pass column out of range: corrupt .ulp"))?;
        let sep1 = self.sep_of(self.sep_base, idx)?;
        let sep2 = self.sep_of(self.sep2_base, idx)?;
        self.url_dict.key_into(url_id as u64, out)?;
        out.push(sep1);
        let user_start = out.len();
        self.user_dict.key_into(user_id as u64, out)?;
        if self.user_dict.reversed {
            out[user_start..].reverse();
        }
        out.push(sep2);
        self.pass_dict.key_into(pass_id as u64, out)?;
        out.push(b'\n');
        Ok(())
    }
}
