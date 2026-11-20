use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use crate::domain::error::DomainError;
use crate::domain::sanitizer::sanitize_for_error;

const AR_GLOBAL_MAGIC: &[u8; 8] = b"!<arch>\n";
const AR_HEADER_SIZE: usize = 60;
const AR_HEADER_TRAILER: [u8; 2] = [0x60, 0x0a];

pub const MAX_AR_MEMBERS: usize = 64;
pub const MAX_AR_MEMBER_SIZE: u64 = 16 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommonArHeader {
    pub identifier: String,
    pub size: u64,
}

pub struct CommonArReader<R> {
    reader: R,
    total_len: u64,
    pos: u64,
    pending_skip: u64,
    member_count: usize,
    source_path: std::path::PathBuf,
}

pub struct CommonArEntry<'a, R> {
    header: CommonArHeader,
    reader: io::Take<&'a mut R>,
    pos_tracker: &'a mut u64,
    pending_skip: &'a mut u64,
    initial_size: u64,
}

impl<R: Read + Seek> CommonArReader<R> {
    pub fn new(mut reader: R, source_path: &Path) -> Result<Self, DomainError> {
        let total_len = reader
            .seek(SeekFrom::End(0))
            .map_err(|err| invalid_deb(source_path, format!("seeking ar archive end: {err}")))?;
        reader
            .seek(SeekFrom::Start(0))
            .map_err(|err| invalid_deb(source_path, format!("rewinding ar archive: {err}")))?;

        let mut magic = [0u8; 8];
        reader
            .read_exact(&mut magic)
            .map_err(|err| invalid_deb(source_path, format!("invalid ar magic header: {err}")))?;
        if &magic != AR_GLOBAL_MAGIC {
            return Err(invalid_deb(
                source_path,
                "invalid ar magic header (expected !<arch>)",
            ));
        }

        Ok(Self {
            reader,
            total_len,
            pos: 8,
            pending_skip: 0,
            member_count: 0,
            source_path: source_path.to_path_buf(),
        })
    }

    pub fn next_entry(&mut self) -> Result<Option<CommonArEntry<'_, R>>, DomainError> {
        if self.pending_skip > 0 {
            let skip_i64 = i64::try_from(self.pending_skip)
                .map_err(|_| invalid_deb(&self.source_path, "ar member offset overflow"))?;
            self.reader
                .seek(SeekFrom::Current(skip_i64))
                .map_err(|err| {
                    invalid_deb(&self.source_path, format!("skipping ar member: {err}"))
                })?;
            self.pos = self
                .pos
                .checked_add(self.pending_skip)
                .ok_or_else(|| invalid_deb(&self.source_path, "ar archive position overflow"))?;
            self.pending_skip = 0;
        }

        if self.pos >= self.total_len {
            return Ok(None);
        }

        let remaining_archive = self.total_len - self.pos;
        if remaining_archive < AR_HEADER_SIZE as u64 {
            return Err(invalid_deb(&self.source_path, "truncated ar member header"));
        }

        if self.member_count >= MAX_AR_MEMBERS {
            return Err(DomainError::ResourceLimitExceeded {
                resource: "ar archive members".to_string(),
                limit: MAX_AR_MEMBERS as u64,
                context: sanitize_for_error(&self.source_path.display().to_string()),
            });
        }

        let mut raw_header = [0u8; AR_HEADER_SIZE];
        self.reader.read_exact(&mut raw_header).map_err(|err| {
            invalid_deb(
                &self.source_path,
                format!("reading ar member header: {err}"),
            )
        })?;
        self.pos = self
            .pos
            .checked_add(AR_HEADER_SIZE as u64)
            .ok_or_else(|| invalid_deb(&self.source_path, "ar archive position overflow"))?;

        let header = parse_common_ar_header(&raw_header, &self.source_path)?;

        if header.size > MAX_AR_MEMBER_SIZE {
            return Err(DomainError::ResourceLimitExceeded {
                resource: "ar member size".to_string(),
                limit: MAX_AR_MEMBER_SIZE,
                context: sanitize_for_error(&header.identifier),
            });
        }

        let member_end = self
            .pos
            .checked_add(header.size)
            .ok_or_else(|| invalid_deb(&self.source_path, "ar member size overflow"))?;
        if member_end > self.total_len {
            return Err(invalid_deb(
                &self.source_path,
                format!(
                    "ar member '{}' size ({}) exceeds archive bounds",
                    header.identifier, header.size
                ),
            ));
        }

        let odd_pad = header.size % 2;
        if odd_pad == 1 && member_end < self.total_len {
            let padded_end = member_end
                .checked_add(1)
                .ok_or_else(|| invalid_deb(&self.source_path, "ar padding overflow"))?;
            if padded_end > self.total_len {
                return Err(invalid_deb(
                    &self.source_path,
                    "truncated ar member alignment byte",
                ));
            }
        }

        self.member_count += 1;
        let initial_size = header.size;
        let total_skip = if member_end < self.total_len {
            initial_size + odd_pad
        } else {
            initial_size
        };
        self.pending_skip = total_skip;

        let take = (&mut self.reader).take(initial_size);
        Ok(Some(CommonArEntry {
            header,
            reader: take,
            pos_tracker: &mut self.pos,
            pending_skip: &mut self.pending_skip,
            initial_size,
        }))
    }
}

impl<R> CommonArEntry<'_, R> {
    pub fn header(&self) -> &CommonArHeader {
        &self.header
    }
}

impl<R: Read> Read for CommonArEntry<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.reader.read(buf)?;
        if n > 0 {
            let delta = n as u64;
            *self.pos_tracker = self
                .pos_tracker
                .checked_add(delta)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "ar offset overflow"))?;
            *self.pending_skip = self.pending_skip.saturating_sub(delta);
        }
        Ok(n)
    }
}

impl<R> Drop for CommonArEntry<'_, R> {
    fn drop(&mut self) {
        let consumed = self.initial_size.saturating_sub(self.reader.limit());
        let _ = consumed;
    }
}

fn parse_common_ar_header(
    raw: &[u8; AR_HEADER_SIZE],
    source_path: &Path,
) -> Result<CommonArHeader, DomainError> {
    if raw[58..60] != AR_HEADER_TRAILER {
        return Err(invalid_deb(source_path, "invalid ar member header trailer"));
    }

    let raw_ident = &raw[0..16];
    if !raw_ident.iter().all(|b| (0x20..=0x7e).contains(b)) {
        return Err(invalid_deb(
            source_path,
            "non-ASCII or control characters in ar member identifier",
        ));
    }

    let ident_str = std::str::from_utf8(raw_ident)
        .map_err(|_| invalid_deb(source_path, "non-UTF8 ar member identifier"))?
        .trim_end_matches(' ');

    if ident_str.starts_with('/') || ident_str.starts_with("#1/") {
        return Err(invalid_deb(
            source_path,
            format!(
                "unsupported extended ar identifier '{}' (only Common ar format is allowed)",
                sanitize_for_error(ident_str)
            ),
        ));
    }

    let cleaned_ident = ident_str.strip_suffix('/').unwrap_or(ident_str);
    if cleaned_ident.is_empty() || cleaned_ident.contains('/') || cleaned_ident.contains('\\') {
        return Err(invalid_deb(
            source_path,
            format!(
                "invalid ar member identifier '{}'",
                sanitize_for_error(ident_str)
            ),
        ));
    }

    let raw_size = std::str::from_utf8(&raw[48..58])
        .map_err(|_| invalid_deb(source_path, "non-UTF8 ar member size"))?
        .trim();
    if raw_size.is_empty() || !raw_size.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid_deb(
            source_path,
            format!("invalid ar member size '{}'", sanitize_for_error(raw_size)),
        ));
    }

    let size = raw_size.parse::<u64>().map_err(|_| {
        invalid_deb(
            source_path,
            format!("ar member size overflow '{}'", sanitize_for_error(raw_size)),
        )
    })?;

    Ok(CommonArHeader {
        identifier: cleaned_ident.to_string(),
        size,
    })
}

fn invalid_deb(path: &Path, reason: impl Into<String>) -> DomainError {
    DomainError::InvalidDebArchive {
        path: path.to_path_buf(),
        reason: sanitize_for_error(&reason.into()),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read};
    use std::path::Path;

    use crate::domain::error::DomainError;

    use super::{CommonArReader, MAX_AR_MEMBERS};

    fn make_ar_entry(ident: &str, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut hdr = [b' '; 60];
        let ident_bytes = ident.as_bytes();
        hdr[..ident_bytes.len().min(16)].copy_from_slice(&ident_bytes[..ident_bytes.len().min(16)]);
        hdr[16..28].copy_from_slice(b"0           ");
        hdr[28..34].copy_from_slice(b"0     ");
        hdr[34..40].copy_from_slice(b"0     ");
        hdr[40..48].copy_from_slice(b"100644  ");
        let size_s = format!("{:<10}", body.len());
        hdr[48..58].copy_from_slice(size_s.as_bytes());
        hdr[58..60].copy_from_slice(b"`\n");
        out.extend_from_slice(&hdr);
        out.extend_from_slice(body);
        if body.len() % 2 == 1 {
            out.push(b'\n');
        }
        out
    }

    #[test]
    fn parses_common_ar_and_rejects_gnu_bsd_extensions_or_oversized_claims()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut valid = b"!<arch>\n".to_vec();
        valid.extend_from_slice(&make_ar_entry("debian-binary", b"2.0\n"));
        valid.extend_from_slice(&make_ar_entry("control.tar.gz/", b"abc"));

        let mut reader = CommonArReader::new(Cursor::new(&valid), Path::new("ok.deb"))?;
        let first = reader
            .next_entry()?
            .ok_or_else(|| DomainError::InvalidControlMetadata("missing first".to_string()))?;
        assert_eq!(first.header().identifier, "debian-binary");
        drop(first);

        let mut second = reader
            .next_entry()?
            .ok_or_else(|| DomainError::InvalidControlMetadata("missing second".to_string()))?;
        assert_eq!(second.header().identifier, "control.tar.gz");
        let mut buf = Vec::new();
        second.read_to_end(&mut buf)?;
        assert_eq!(buf, b"abc");
        drop(second);
        assert!(reader.next_entry()?.is_none());

        let mut gnu_long = b"!<arch>\n".to_vec();
        gnu_long.extend_from_slice(&make_ar_entry("//", b"longnames"));
        let mut r_gnu = CommonArReader::new(Cursor::new(&gnu_long), Path::new("gnu.deb"))?;
        assert!(r_gnu.next_entry().is_err());

        let mut bsd_long = b"!<arch>\n".to_vec();
        bsd_long.extend_from_slice(&make_ar_entry("#1/12", b"control.tar!"));
        let mut r_bsd = CommonArReader::new(Cursor::new(&bsd_long), Path::new("bsd.deb"))?;
        assert!(r_bsd.next_entry().is_err());

        let mut huge_claim = b"!<arch>\n".to_vec();
        let mut hdr = [b' '; 60];
        hdr[..13].copy_from_slice(b"debian-binary");
        hdr[48..58].copy_from_slice(b"9999999999");
        hdr[58..60].copy_from_slice(b"`\n");
        huge_claim.extend_from_slice(&hdr);
        let mut r_huge = CommonArReader::new(Cursor::new(&huge_claim), Path::new("huge.deb"))?;
        assert!(r_huge.next_entry().is_err());

        let mut too_many = b"!<arch>\n".to_vec();
        for idx in 0..=MAX_AR_MEMBERS {
            too_many.extend_from_slice(&make_ar_entry(&format!("m{idx}"), b""));
        }
        let mut r_many = CommonArReader::new(Cursor::new(&too_many), Path::new("many.deb"))?;
        let mut err = None;
        for _ in 0..=MAX_AR_MEMBERS {
            match r_many.next_entry() {
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(e) => {
                    err = Some(e);
                    break;
                }
            }
        }
        assert!(matches!(
            err,
            Some(DomainError::ResourceLimitExceeded { .. })
        ));
        Ok(())
    }
}
