//! C ABI v1. Valid handles and readable pointer/length pairs are caller obligations.
//! All functions catch Rust panics. Only producer submission supports concurrent calls.
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
        let rs = copy_pairs(rows, n)?;
        w.write_pairs_owned(rs)?;
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
        let rs = copy_alignments(rows, n)?;
        w.write_reads_owned(rs, &[0, n])?;
        Ok(0)
    })
}
/// Submit multiple complete reads with offsets [0, ..., n].
/// # Safety
/// `w` must be a live exclusively accessed writer. `rows` and `offsets` must
/// contain `n` and `offset_count` initialized entries respectively. All filter
/// strings must be readable and NUL-terminated for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn pqsio_write_reads(
    w: *mut Writer,
    rows: *const CAlignment,
    n: usize,
    offsets: *const usize,
    offset_count: usize,
) -> i32 {
    call(|| {
        let w = w.as_mut().context("null writer")?;
        let offsets = slice(offsets, offset_count)?;
        let rs = copy_alignments(rows, n)?;
        w.write_reads_owned(rs, offsets)?;
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
            Some(Batch::Pairs(mut rows)) => {
                let names = rows
                    .iter_mut()
                    .map(|r| CString::new(std::mem::take(&mut r.read_id)))
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
            Some(Batch::Concat(mut rows)) => {
                let names = rows
                    .iter_mut()
                    .map(|r| CString::new(std::mem::take(&mut r.filter_reason)))
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

unsafe fn copy_pairs(rows: *const CPair, n: usize) -> Result<Vec<Pair>> {
    slice(rows, n)?
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
        .collect::<Result<Vec<_>>>()
}

unsafe fn copy_alignments(rows: *const CAlignment, n: usize) -> Result<Vec<Alignment>> {
    slice(rows, n)?
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
        .collect::<Result<Vec<_>>>()
}

/// # Safety
/// Same path/contig/output pointer requirements as pqsio_writer_open.
#[no_mangle]
pub unsafe extern "C" fn pqsio_parallel_open(
    path: *const c_char,
    kind: u32,
    contigs: *const CContig,
    n: usize,
    chunk_size: usize,
    workers: usize,
    queue_capacity: usize,
    max_batch_bytes: usize,
    out: *mut *mut ParallelWriter,
) -> i32 {
    call(|| {
        ensure!(!out.is_null(), "null output handle");
        *out = ptr::null_mut();
        let kind = match kind {
            0 => Kind::Pairs,
            1 => Kind::Concat,
            _ => bail!("kind must be 0 or 1"),
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
        *out = Box::into_raw(Box::new(ParallelWriter::create(
            text(path)?,
            kind,
            cs,
            chunk_size,
            ParallelOptions {
                workers,
                queue_capacity,
                max_batch_bytes,
            },
        )?));
        Ok(0)
    })
}
/// # Safety
/// Writer must remain live and not be finished/destroyed during this call.
/// Out must be writable and not hold an existing owned producer.
#[no_mangle]
pub unsafe extern "C" fn pqsio_parallel_producer(
    w: *const ParallelWriter,
    out: *mut *mut Producer,
) -> i32 {
    call(|| {
        ensure!(!out.is_null(), "null output handle");
        *out = ptr::null_mut();
        *out = Box::into_raw(Box::new(
            w.as_ref().context("null parallel writer")?.producer(),
        ));
        Ok(0)
    })
}
/// # Safety
/// Producer must remain live for the call. Concurrent submissions are allowed.
/// Rows and their strings must be readable for the call, as with write_pairs.
#[no_mangle]
pub unsafe extern "C" fn pqsio_producer_pairs(
    p: *const Producer,
    sequence: u64,
    rows: *const CPair,
    n: usize,
) -> i32 {
    call(|| {
        p.as_ref()
            .context("null producer")?
            .write_pairs(sequence, copy_pairs(rows, n)?)?;
        Ok(0)
    })
}
/// # Safety
/// Producer must remain live. Concurrent submissions are allowed. Rows,
/// offsets and strings must be readable as with pqsio_write_reads.
#[no_mangle]
pub unsafe extern "C" fn pqsio_producer_reads(
    p: *const Producer,
    sequence: u64,
    rows: *const CAlignment,
    n: usize,
    offsets: *const usize,
    offset_count: usize,
) -> i32 {
    call(|| {
        p.as_ref().context("null producer")?.write_reads(
            sequence,
            copy_alignments(rows, n)?,
            slice(offsets, offset_count)?.to_vec(),
        )?;
        Ok(0)
    })
}
/// # Safety
/// Writer must be live and exclusively accessed. Producers can remain live.
#[no_mangle]
pub unsafe extern "C" fn pqsio_parallel_finish(w: *mut ParallelWriter) -> i32 {
    call(|| {
        w.as_mut().context("null parallel writer")?.finish()?;
        Ok(0)
    })
}
/// # Safety
/// Writer must be null or live and exclusively owned; becomes invalid.
/// This aborts unfinished work and waits for workers. Producers remain valid.
#[no_mangle]
pub unsafe extern "C" fn pqsio_parallel_destroy(w: *mut ParallelWriter) -> i32 {
    call(|| {
        if !w.is_null() {
            drop(Box::from_raw(w));
        }
        Ok(0)
    })
}
/// # Safety
/// Producer must be null or live with no calls in progress; becomes invalid.
#[no_mangle]
pub unsafe extern "C" fn pqsio_producer_destroy(p: *mut Producer) -> i32 {
    call(|| {
        if !p.is_null() {
            drop(Box::from_raw(p));
        }
        Ok(0)
    })
}

// Additive columnar extension. The original ABI remains version 1.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Spanu8 {
    pub data: *const u8,
    pub len: usize,
}
impl Spanu8 {
    unsafe fn borrow<'a>(&self) -> Result<&'a [u8]> {
        ensure!(
            self.len == 0 || self.data.is_aligned(),
            "unaligned column buffer"
        );
        slice(self.data, self.len)
    }
    fn view(v: &[u8]) -> Self {
        Self {
            data: v.as_ptr(),
            len: v.len(),
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Spanu32 {
    pub data: *const u32,
    pub len: usize,
}
impl Spanu32 {
    unsafe fn borrow<'a>(&self) -> Result<&'a [u32]> {
        ensure!(
            self.len == 0 || self.data.is_aligned(),
            "unaligned column buffer"
        );
        slice(self.data, self.len)
    }
    fn view(v: &[u32]) -> Self {
        Self {
            data: v.as_ptr(),
            len: v.len(),
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Spanu64 {
    pub data: *const u64,
    pub len: usize,
}
impl Spanu64 {
    unsafe fn borrow<'a>(&self) -> Result<&'a [u64]> {
        ensure!(
            self.len == 0 || self.data.is_aligned(),
            "unaligned column buffer"
        );
        slice(self.data, self.len)
    }
    fn view(v: &[u64]) -> Self {
        Self {
            data: v.as_ptr(),
            len: v.len(),
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Spanf32 {
    pub data: *const f32,
    pub len: usize,
}
impl Spanf32 {
    unsafe fn borrow<'a>(&self) -> Result<&'a [f32]> {
        ensure!(
            self.len == 0 || self.data.is_aligned(),
            "unaligned column buffer"
        );
        slice(self.data, self.len)
    }
    fn view(v: &[f32]) -> Self {
        Self {
            data: v.as_ptr(),
            len: v.len(),
        }
    }
}
#[repr(C)]
pub struct CPairColumns {
    pub read_id_offsets: Spanu64,
    pub read_id_bytes: Spanu8,
    pub chrom1: Spanu32,
    pub pos1: Spanu64,
    pub chrom2: Spanu32,
    pub pos2: Spanu64,
    pub strand1: Spanu8,
    pub strand2: Spanu8,
    pub mapq: Spanu8,
}
impl CPairColumns {
    unsafe fn borrow(&self) -> Result<PairColumnsView<'_>> {
        Ok(PairColumnsView {
            read_id_offsets: self.read_id_offsets.borrow()?,
            read_id_bytes: self.read_id_bytes.borrow()?,
            chrom1: self.chrom1.borrow()?,
            pos1: self.pos1.borrow()?,
            chrom2: self.chrom2.borrow()?,
            pos2: self.pos2.borrow()?,
            strand1: self.strand1.borrow()?,
            strand2: self.strand2.borrow()?,
            mapq: self.mapq.borrow()?,
        })
    }
    fn view(v: &PairColumns) -> Self {
        Self {
            read_id_offsets: Spanu64::view(&v.read_id_offsets),
            read_id_bytes: Spanu8::view(&v.read_id_bytes),
            chrom1: Spanu32::view(&v.chrom1),
            pos1: Spanu64::view(&v.pos1),
            chrom2: Spanu32::view(&v.chrom2),
            pos2: Spanu64::view(&v.pos2),
            strand1: Spanu8::view(&v.strand1),
            strand2: Spanu8::view(&v.strand2),
            mapq: Spanu8::view(&v.mapq),
        }
    }
}
/// # Safety
/// See the columnar pointer, lifetime and exclusivity contract in pqsio.h.
#[no_mangle]
pub unsafe extern "C" fn pqsio_write_pairs_columns(w: *mut Writer, v: *const CPairColumns) -> i32 {
    call(|| {
        w.as_mut()
            .context("null writer")?
            .write_pairs_columns(v.as_ref().context("null columns")?.borrow()?)?;
        Ok(0)
    })
}
/// # Safety
/// Batch must be live; out must be writable and naturally aligned.
#[no_mangle]
pub unsafe extern "C" fn pqsio_column_batch_pairs(
    b: *const ColumnBatch,
    out: *mut CPairColumns,
) -> i32 {
    call(|| {
        ensure!(!out.is_null(), "null view output");
        let ColumnBatch::Pairs(v) = b.as_ref().context("null column batch")? else {
            bail!("wrong column batch kind");
        };
        *out = CPairColumns::view(v);
        Ok(0)
    })
}
#[repr(C)]
pub struct CConcatColumns {
    pub read_offsets: Spanu64,
    pub read_idx: Spanu64,
    pub read_length: Spanu32,
    pub read_start: Spanu32,
    pub read_end: Spanu32,
    pub strand: Spanu8,
    pub chrom: Spanu32,
    pub start: Spanu64,
    pub end: Spanu64,
    pub mapping_quality: Spanu8,
    pub identity: Spanf32,
    pub filter_reason_offsets: Spanu64,
    pub filter_reason_bytes: Spanu8,
}
impl CConcatColumns {
    unsafe fn borrow(&self) -> Result<ConcatColumnsView<'_>> {
        Ok(ConcatColumnsView {
            read_offsets: self.read_offsets.borrow()?,
            read_idx: self.read_idx.borrow()?,
            read_length: self.read_length.borrow()?,
            read_start: self.read_start.borrow()?,
            read_end: self.read_end.borrow()?,
            strand: self.strand.borrow()?,
            chrom: self.chrom.borrow()?,
            start: self.start.borrow()?,
            end: self.end.borrow()?,
            mapping_quality: self.mapping_quality.borrow()?,
            identity: self.identity.borrow()?,
            filter_reason_offsets: self.filter_reason_offsets.borrow()?,
            filter_reason_bytes: self.filter_reason_bytes.borrow()?,
        })
    }
    fn view(v: &ConcatColumns) -> Self {
        Self {
            read_offsets: Spanu64::view(&v.read_offsets),
            read_idx: Spanu64::view(&v.read_idx),
            read_length: Spanu32::view(&v.read_length),
            read_start: Spanu32::view(&v.read_start),
            read_end: Spanu32::view(&v.read_end),
            strand: Spanu8::view(&v.strand),
            chrom: Spanu32::view(&v.chrom),
            start: Spanu64::view(&v.start),
            end: Spanu64::view(&v.end),
            mapping_quality: Spanu8::view(&v.mapping_quality),
            identity: Spanf32::view(&v.identity),
            filter_reason_offsets: Spanu64::view(&v.filter_reason_offsets),
            filter_reason_bytes: Spanu8::view(&v.filter_reason_bytes),
        }
    }
}
/// # Safety
/// See the columnar pointer, lifetime and exclusivity contract in pqsio.h.
#[no_mangle]
pub unsafe extern "C" fn pqsio_write_concat_columns(
    w: *mut Writer,
    v: *const CConcatColumns,
) -> i32 {
    call(|| {
        w.as_mut()
            .context("null writer")?
            .write_concat_columns(v.as_ref().context("null columns")?.borrow()?)?;
        Ok(0)
    })
}
/// # Safety
/// Batch must be live; out must be writable and naturally aligned.
#[no_mangle]
pub unsafe extern "C" fn pqsio_column_batch_concat(
    b: *const ColumnBatch,
    out: *mut CConcatColumns,
) -> i32 {
    call(|| {
        ensure!(!out.is_null(), "null view output");
        let ColumnBatch::Concat(v) = b.as_ref().context("null column batch")? else {
            bail!("wrong column batch kind");
        };
        *out = CConcatColumns::view(v);
        Ok(0)
    })
}
#[no_mangle]
pub extern "C" fn pqsio_columnar_version() -> u32 {
    1
}
/// # Safety
/// Reader must be live and exclusive; out must be writable, not a live owner.
#[no_mangle]
pub unsafe extern "C" fn pqsio_reader_next_columns(
    r: *mut Reader,
    out: *mut *mut ColumnBatch,
) -> i32 {
    call(|| {
        ensure!(!out.is_null(), "null batch output");
        *out = ptr::null_mut();
        match r.as_mut().context("null reader")?.next_columns()? {
            Some(b) => {
                *out = Box::into_raw(Box::new(b));
                Ok(1)
            }
            None => Ok(0),
        }
    })
}
/// # Safety
/// b must be NULL or a live batch returned by next_columns; destroy exactly once.
#[no_mangle]
pub unsafe extern "C" fn pqsio_column_batch_destroy(b: *mut ColumnBatch) -> i32 {
    call(|| {
        if !b.is_null() {
            drop(Box::from_raw(b));
        }
        Ok(0)
    })
}
