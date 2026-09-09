//! C ABI v1. Valid handles and readable pointer/length pairs are caller obligations.
//! All functions catch Rust panics. Handles must not be used concurrently.
use crate::*;
use std::{
    cell::RefCell,
    ffi::{c_char, c_void, CStr, CString},
    panic::{catch_unwind, AssertUnwindSafe},
    ptr,
};
thread_local! { static ERROR: RefCell<CString> = RefCell::new(CString::default()); }
fn call(f: impl FnOnce() -> Result<i32>) -> i32 {
    ERROR.with(|e| *e.borrow_mut() = CString::default());
    let result = catch_unwind(AssertUnwindSafe(f))
        .unwrap_or_else(|_| Err(anyhow::anyhow!("Rust panic at PQS ABI boundary")));
    match result {
        Ok(n) => n,
        Err(e) => {
            ERROR.with(|s| {
                *s.borrow_mut() = CString::new(format!("{e:#}").replace('\0', "?")).unwrap()
            });
            -1
        }
    }
}
unsafe fn text(p: *const c_char) -> Result<String> {
    ensure!(!p.is_null(), "null string pointer");
    Ok(CStr::from_ptr(p).to_str()?.into())
}
unsafe fn slice<'a, T>(p: *const T, n: usize) -> Result<&'a [T]> {
    if n == 0 {
        return Ok(&[]);
    }
    ensure!(!p.is_null(), "null array pointer");
    ensure!(
        n <= isize::MAX as usize / std::mem::size_of::<T>().max(1),
        "array too large"
    );
    Ok(std::slice::from_raw_parts(p, n))
}
#[repr(C)]
pub struct CContig {
    pub name: *const c_char,
    pub length: u64,
}
#[repr(C)]
pub struct CPair {
    pub read_id: *const c_char,
    pub chrom1: u32,
    pub pos1: u64,
    pub chrom2: u32,
    pub pos2: u64,
    pub strand1: u8,
    pub strand2: u8,
    pub mapq: u8,
}
#[repr(C)]
pub struct CAlignment {
    pub read_idx: u64,
    pub read_length: u32,
    pub read_start: u32,
    pub read_end: u32,
    pub strand: u8,
    pub chrom: u32,
    pub start: u64,
    pub end: u64,
    pub mapping_quality: u8,
    pub identity: f32,
    pub filter_reason: *const c_char,
}
#[no_mangle]
pub extern "C" fn pqsio_abi_version() -> u32 {
    1
}
#[no_mangle]
pub extern "C" fn pqsio_last_error() -> *const c_char {
    ERROR.with(|e| e.borrow().as_ptr())
}
/// # Safety
/// `path` and contig names must be readable NUL-terminated strings; the contig
/// array must contain `n` initialized entries. `out` must be writable and must
/// not contain an owned live handle that would be overwritten.
#[no_mangle]
pub unsafe extern "C" fn pqsio_writer_open(
    path: *const c_char,
    kind: u32,
    contigs: *const CContig,
    n: usize,
    chunk_size: usize,
    out: *mut *mut Writer,
) -> i32 {
    call(|| {
        ensure!(!out.is_null(), "null output handle");
        *out = ptr::null_mut();
        let kind = match kind {
            0 => Kind::Pairs,
            1 => Kind::Concat,
            _ => bail!("kind must be 0 (pairs) or 1 (concat)"),
        };
        let cs = slice(contigs, n)?
            .iter()
            .map(|c| {
                Ok(Contig {
                    name: text(c.name)?,
                    length: c.length,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        *out = Box::into_raw(Box::new(Writer::create(text(path)?, kind, cs, chunk_size)?));
        Ok(0)
    })
}
/// # Safety
/// `w` must be a live exclusively accessed writer; `rows` must contain `n`
/// initialized entries with readable NUL-terminated read IDs for this call.
#[no_mangle]
pub unsafe extern "C" fn pqsio_write_pairs(w: *mut Writer, rows: *const CPair, n: usize) -> i32 {
    call(|| {
        let w = w.as_mut().context("null writer")?;
        let rs = slice(rows, n)?
            .iter()
            .map(|r| {
                Ok(Pair {
                    read_id: text(r.read_id)?,
                    chrom1: r.chrom1,
                    pos1: r.pos1,
                    chrom2: r.chrom2,
                    pos2: r.pos2,
                    strand1: r.strand1,
                    strand2: r.strand2,
                    mapq: r.mapq,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        w.write_pairs(&rs)?;
        Ok(0)
    })
}
/// # Safety
/// `w` must be a live exclusively accessed writer; `rows` must contain `n`
/// initialized entries with readable NUL-terminated filter strings for this call.
#[no_mangle]
pub unsafe extern "C" fn pqsio_write_read(
    w: *mut Writer,
    rows: *const CAlignment,
    n: usize,
) -> i32 {
    call(|| {
        let w = w.as_mut().context("null writer")?;
        let rs = slice(rows, n)?
            .iter()
            .map(|r| {
                Ok(Alignment {
                    read_idx: r.read_idx,
                    read_length: r.read_length,
                    read_start: r.read_start,
                    read_end: r.read_end,
                    strand: r.strand,
                    chrom: r.chrom,
                    start: r.start,
                    end: r.end,
                    mapping_quality: r.mapping_quality,
                    identity: r.identity,
                    filter_reason: text(r.filter_reason)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        w.write_read(&rs)?;
        Ok(0)
    })
}
/// # Safety
/// `w` must be a live exclusively accessed writer handle returned by this ABI.
#[no_mangle]
pub unsafe extern "C" fn pqsio_writer_finish(w: *mut Writer) -> i32 {
    call(|| {
        w.as_mut().context("null writer")?.finish()?;
        Ok(0)
    })
}
/// # Safety
/// `w` must be null or a live exclusively owned handle returned by this ABI.
/// The handle becomes invalid and must never be used or freed again.
#[no_mangle]
pub unsafe extern "C" fn pqsio_writer_destroy(w: *mut Writer) -> i32 {
    call(|| {
        if !w.is_null() {
            drop(Box::from_raw(w));
        }
        Ok(0)
    })
}
/// # Safety
/// `path` must be a readable NUL-terminated string. `out` must be writable
/// and must not contain an owned live handle that would be overwritten.
#[no_mangle]
pub unsafe extern "C" fn pqsio_reader_open(
    path: *const c_char,
    min_mapq: u8,
    out: *mut *mut Reader,
) -> i32 {
    call(|| {
        ensure!(!out.is_null(), "null output handle");
        *out = ptr::null_mut();
        *out = Box::into_raw(Box::new(Reader::open(text(path)?, min_mapq)?));
        Ok(0)
    })
}
/// # Safety
/// `r` must be a live reader handle with no concurrent mutation or destruction.
#[no_mangle]
pub unsafe extern "C" fn pqsio_reader_kind(r: *const Reader) -> i32 {
    call(|| {
        Ok(match r.as_ref().context("null reader")?.kind {
            Kind::Pairs => 0,
            Kind::Concat => 1,
        })
    })
}
pub type ContigsCallback = unsafe extern "C" fn(*const CContig, usize, *mut c_void) -> i32;
pub type PairsCallback = unsafe extern "C" fn(*const CPair, usize, *mut c_void) -> i32;
pub type ConcatCallback = unsafe extern "C" fn(*const CAlignment, usize, *mut c_void) -> i32;
/// # Safety
/// `r` must be live and not concurrently accessed. The callback must not
/// unwind, destroy/re-enter the reader, or retain borrowed pointers. `user`
/// must satisfy the callback's own pointer validity requirements.
#[no_mangle]
pub unsafe extern "C" fn pqsio_reader_contigs(
    r: *const Reader,
    cb: Option<ContigsCallback>,
    user: *mut c_void,
) -> i32 {
    call(|| {
        let r = r.as_ref().context("null reader")?;
        let cb = cb.context("null contigs callback")?;
        let strings = r
            .contigs
            .iter()
            .map(|c| CString::new(c.name.as_str()))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let cs = r
            .contigs
            .iter()
            .zip(&strings)
            .map(|(c, s)| CContig {
                name: s.as_ptr(),
                length: c.length,
            })
            .collect::<Vec<_>>();
        ensure!(
            cb(cs.as_ptr(), cs.len(), user) == 0,
            "contigs callback failed"
        );
        Ok(0)
    })
}
/// 1 = delivered a shard, 0 = EOF, -1 = error. Callback memory is borrowed
/// only for the duration of the callback; copy anything retained by the caller.
/// # Safety
/// `r` must be live and exclusively accessed. Callbacks must not unwind,
/// destroy/re-enter the reader, or retain borrowed pointers. `user` must
/// satisfy each callback's own pointer validity requirements.
#[no_mangle]
pub unsafe extern "C" fn pqsio_reader_next(
    r: *mut Reader,
    pairs: Option<PairsCallback>,
    concat: Option<ConcatCallback>,
    user: *mut c_void,
) -> i32 {
    call(|| {
        let r = r.as_mut().context("null reader")?;
        ensure!(
            if r.kind == Kind::Pairs {
                pairs.is_some()
            } else {
                concat.is_some()
            },
            "missing callback for dataset kind"
        );
        match r.next_batch()? {
            None => return Ok(0),
            Some(Batch::Pairs(rows)) => {
                let names = rows
                    .iter()
                    .map(|r| CString::new(r.read_id.as_str()))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                let rs = rows
                    .iter()
                    .zip(&names)
                    .map(|(r, s)| CPair {
                        read_id: s.as_ptr(),
                        chrom1: r.chrom1,
                        pos1: r.pos1,
                        chrom2: r.chrom2,
                        pos2: r.pos2,
                        strand1: r.strand1,
                        strand2: r.strand2,
                        mapq: r.mapq,
                    })
                    .collect::<Vec<_>>();
                ensure!(
                    pairs.unwrap()(rs.as_ptr(), rs.len(), user) == 0,
                    "pairs callback failed"
                );
            }
            Some(Batch::Concat(rows)) => {
                let names = rows
                    .iter()
                    .map(|r| CString::new(r.filter_reason.as_str()))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                let rs = rows
                    .iter()
                    .zip(&names)
                    .map(|(r, s)| CAlignment {
                        read_idx: r.read_idx,
                        read_length: r.read_length,
                        read_start: r.read_start,
                        read_end: r.read_end,
                        strand: r.strand,
                        chrom: r.chrom,
                        start: r.start,
                        end: r.end,
                        mapping_quality: r.mapping_quality,
                        identity: r.identity,
                        filter_reason: s.as_ptr(),
                    })
                    .collect::<Vec<_>>();
                ensure!(
                    concat.unwrap()(rs.as_ptr(), rs.len(), user) == 0,
                    "concat callback failed"
                );
            }
        }
        Ok(1)
    })
}
/// # Safety
/// `r` must be null or a live exclusively owned handle returned by this ABI.
/// The handle becomes invalid and must never be used or freed again.
#[no_mangle]
pub unsafe extern "C" fn pqsio_reader_destroy(r: *mut Reader) -> i32 {
    call(|| {
        if !r.is_null() {
            drop(Box::from_raw(r));
        }
        Ok(0)
    })
}
