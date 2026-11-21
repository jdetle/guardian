//! Listing a directory together with each entry's size.
//!
//! macOS answers "what is in this directory, and how big is each file" for many entries per
//! system call (`getattrlistbulk`), where the portable way costs one call per file. On APFS
//! it also says how much of a file's space is its own: files cloned from one another
//! (`cp -c`, a bun or pnpm install from its cache) share their blocks, and deleting one copy
//! gives nothing back.

use std::ffi::{CString, OsString};
use std::fs;
use std::os::fd::{FromRawFd, OwnedFd, AsRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// One directory entry. Symlinks are reported as themselves, never followed.
#[derive(Debug)]
pub struct Entry {
    pub name: OsString,
    pub is_dir: bool,
    /// The volume it lives on, as `st_dev` reports it.
    pub device: u64,
    /// Space allocated to it. Zero for directories: their contents are listed separately.
    pub allocated: u64,
    /// Space that deleting it would free at once: `allocated`, less whatever it shares with
    /// a clone or a snapshot. Equal to `allocated` where the file system cannot say. A file
    /// written within the last moment can read low.
    pub private: u64,
    /// How many names (hard links) the file has.
    pub links: u64,
    pub inode: u64,
}

// <sys/attr.h>, <sys/vnode.h>. Declared here rather than taken from the libc crate, whose
// coverage of these has changed between releases.
const ATTR_BIT_MAP_COUNT: u16 = 5;
const ATTR_CMN_NAME: u32 = 0x0000_0001;
const ATTR_CMN_DEVID: u32 = 0x0000_0002;
const ATTR_CMN_OBJTYPE: u32 = 0x0000_0008;
const ATTR_CMN_FILEID: u32 = 0x0200_0000;
const ATTR_CMN_ERROR: u32 = 0x2000_0000;
const ATTR_CMN_RETURNED_ATTRS: u32 = 0x8000_0000;
const ATTR_FILE_LINKCOUNT: u32 = 0x0000_0001;
const ATTR_FILE_ALLOCSIZE: u32 = 0x0000_0004;
const ATTR_CMNEXT_PRIVATESIZE: u32 = 0x0000_0008;
/// Makes the "fork" attribute group mean the extended common attributes (private size).
const FSOPT_ATTR_CMN_EXTENDED: u64 = 0x20;
const VDIR: u32 = 2;

#[repr(C)]
struct AttrList {
    bitmapcount: u16,
    reserved: u16,
    commonattr: u32,
    volattr: u32,
    dirattr: u32,
    fileattr: u32,
    forkattr: u32,
}

extern "C" {
    fn getattrlistbulk(
        dirfd: libc::c_int,
        attr_list: *mut AttrList,
        attr_buf: *mut libc::c_void,
        attr_buf_size: libc::size_t,
        options: u64,
    ) -> libc::c_int;
}

const BUFFER_BYTES: usize = 128 * 1024;

/// Reads fixed-size fields out of one packed entry, refusing to run past its end.
struct Fields<'a> {
    data: &'a [u8],
    at: usize,
}

impl Fields<'_> {
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let bytes = self.data.get(self.at..self.at.checked_add(N)?)?;
        self.at += N;
        bytes.try_into().ok()
    }

    fn u32(&mut self) -> Option<u32> {
        self.take().map(u32::from_ne_bytes)
    }

    fn i32(&mut self) -> Option<i32> {
        self.take().map(i32::from_ne_bytes)
    }

    fn u64(&mut self) -> Option<u64> {
        self.take().map(u64::from_ne_bytes)
    }

    /// A size the kernel reports as a signed `off_t`.
    fn size(&mut self) -> Option<u64> {
        self.take().map(|b| i64::from_ne_bytes(b).max(0) as u64)
    }
}

/// Decode one entry of a `getattrlistbulk` buffer. `None` for an entry the kernel could not
/// read, or one that is malformed.
fn parse_entry(data: &[u8]) -> Option<Entry> {
    // Attributes follow in a fixed order: the entry's length, which attributes were
    // returned, the error if there was one, then each group in ascending bit order.
    let mut f = Fields { data, at: 4 };
    let common = f.u32()?;
    let _volume = f.u32()?;
    let _dir = f.u32()?;
    let file = f.u32()?;
    let fork = f.u32()?;
    if common & ATTR_CMN_ERROR != 0 && f.u32()? != 0 {
        return None;
    }
    if common & ATTR_CMN_NAME == 0 {
        return None;
    }
    // A reference: where the name starts, counted from the reference itself, and its
    // length including the terminating NUL.
    let reference_at = f.at;
    let offset = f.i32()?;
    let length = f.u32()? as usize;
    let start = reference_at.checked_add(usize::try_from(offset).ok()?)?;
    let raw = data.get(start..start.checked_add(length)?)?;
    let name = raw.split(|b| *b == 0).next()?;
    if name.is_empty() {
        return None;
    }
    let device = if common & ATTR_CMN_DEVID != 0 {
        // Widened the way `st_dev` is, so the two compare equal.
        f.i32()? as u64
    } else {
        0
    };
    let is_dir = common & ATTR_CMN_OBJTYPE != 0 && f.u32()? == VDIR;
    let inode = if common & ATTR_CMN_FILEID != 0 { f.u64()? } else { 0 };
    let links = if file & ATTR_FILE_LINKCOUNT != 0 { u64::from(f.u32()?) } else { 1 };
    let allocated = if file & ATTR_FILE_ALLOCSIZE != 0 { f.size()? } else { 0 };
    let private = if fork & ATTR_CMNEXT_PRIVATESIZE != 0 {
        f.size()?.min(allocated)
    } else {
        allocated
    };
    Some(Entry {
        name: OsString::from_vec(name.to_vec()),
        is_dir,
        device,
        allocated: if is_dir { 0 } else { allocated },
        private: if is_dir { 0 } else { private },
        links,
        inode,
    })
}

/// List `dir` with `getattrlistbulk`. `None` when the directory cannot be opened, the
/// volume does not support the call, or it failed before the end; `out` may then hold a
/// partial listing.
fn list_bulk(dir: &Path, buf: &mut Vec<u8>, out: &mut Vec<Entry>) -> Option<()> {
    let path = CString::new(dir.as_os_str().as_bytes()).ok()?;
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return None;
    }
    // Closed when dropped.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    buf.resize(BUFFER_BYTES, 0);
    let mut wanted = AttrList {
        bitmapcount: ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: ATTR_CMN_RETURNED_ATTRS
            | ATTR_CMN_ERROR
            | ATTR_CMN_NAME
            | ATTR_CMN_DEVID
            | ATTR_CMN_OBJTYPE
            | ATTR_CMN_FILEID,
        volattr: 0,
        dirattr: 0,
        fileattr: ATTR_FILE_LINKCOUNT | ATTR_FILE_ALLOCSIZE,
        forkattr: ATTR_CMNEXT_PRIVATESIZE,
    };
    let mut first = true;
    loop {
        let count = unsafe {
            getattrlistbulk(
                fd.as_raw_fd(),
                &mut wanted,
                buf.as_mut_ptr().cast(),
                buf.len(),
                FSOPT_ATTR_CMN_EXTENDED,
            )
        };
        if count < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            // Refused, at once or part-way through: the caller lists it the portable way
            // from the start, rather than take a part for the whole.
            let _ = first;
            return None;
        }
        if count == 0 {
            return Some(());
        }
        first = false;
        let mut at = 0usize;
        for _ in 0..count {
            let Some(length) = buf
                .get(at..at + 4)
                .and_then(|b| b.try_into().ok())
                .map(u32::from_ne_bytes)
            else {
                return Some(());
            };
            let length = length as usize;
            let Some(data) = buf.get(at..at.saturating_add(length)) else {
                return Some(());
            };
            if length < 4 {
                return Some(());
            }
            if let Some(entry) = parse_entry(data) {
                out.push(entry);
            }
            at += length;
        }
    }
}

/// List `dir` with one `lstat` per entry: works on any file system, but cannot tell what a
/// file shares with a clone.
fn list_portable(dir: &Path, out: &mut Vec<Entry>) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        // DirEntry::metadata does not follow symlinks.
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let is_dir = meta.file_type().is_dir();
        let allocated = if is_dir { 0 } else { meta.blocks().saturating_mul(512) };
        out.push(Entry {
            name: entry.file_name(),
            is_dir,
            device: meta.dev(),
            allocated,
            private: allocated,
            links: meta.nlink(),
            inode: meta.ino(),
        });
    }
    true
}

/// Replace `out` with the entries of `dir`. `buf` is scratch space kept between calls.
/// False when the directory could not be read.
pub fn list(dir: &Path, buf: &mut Vec<u8>, out: &mut Vec<Entry>) -> bool {
    out.clear();
    if list_bulk(dir, buf, out).is_some() {
        return true;
    }
    out.clear();
    list_portable(dir, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("guardian-list-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(fs::canonicalize(&dir).unwrap())
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn sorted(mut entries: Vec<Entry>) -> Vec<Entry> {
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        entries
    }

    fn fill(s: &Scratch) {
        fs::write(s.0.join("file.bin"), vec![b'x'; 300_000]).unwrap();
        fs::create_dir(s.0.join("sub")).unwrap();
        fs::write(s.0.join("sub/inner.bin"), vec![b'x'; 900_000]).unwrap();
        std::os::unix::fs::symlink(s.0.join("sub"), s.0.join("link")).unwrap();
        fs::hard_link(s.0.join("file.bin"), s.0.join("twin.bin")).unwrap();
    }

    fn check(entries: Vec<Entry>, device: u64) {
        let entries = sorted(entries);
        let names: Vec<&str> = entries.iter().map(|e| e.name.to_str().unwrap()).collect();
        assert_eq!(names, vec!["file.bin", "link", "sub", "twin.bin"]);
        let (file, link, sub, twin) = (&entries[0], &entries[1], &entries[2], &entries[3]);
        assert!(!file.is_dir && sub.is_dir);
        assert!(!link.is_dir, "a symlink to a directory is not a directory to walk");
        assert!((300_000..600_000).contains(&file.allocated), "{}", file.allocated);
        assert!(file.private <= file.allocated);
        assert_eq!(sub.allocated, 0, "a directory's contents are not counted in its own entry");
        assert_eq!((file.links, twin.links), (2, 2));
        assert_eq!(file.inode, twin.inode);
        assert_ne!(file.inode, sub.inode);
        assert!(entries.iter().all(|e| e.device == device));
    }

    #[test]
    fn both_ways_of_listing_agree() {
        let s = Scratch::new("agree");
        fill(&s);
        let device = fs::metadata(&s.0).unwrap().dev();

        let mut portable = Vec::new();
        assert!(list_portable(&s.0, &mut portable));
        check(portable, device);

        let (mut buf, mut bulk) = (Vec::new(), Vec::new());
        assert!(list_bulk(&s.0, &mut buf, &mut bulk).is_some(), "the temp volume supports bulk listing");
        check(bulk, device);
    }

    #[test]
    fn a_directory_with_many_entries_is_listed_whole() {
        // More entries than one buffer holds, so the listing takes several calls.
        let s = Scratch::new("many");
        let long = "n".repeat(120);
        for i in 0..2_500 {
            fs::write(s.0.join(format!("{long}-{i}")), b"").unwrap();
        }
        let (mut buf, mut out) = (Vec::new(), Vec::new());
        assert!(list(&s.0, &mut buf, &mut out));
        assert_eq!(out.len(), 2_500);
        let unique: std::collections::HashSet<&OsString> = out.iter().map(|e| &e.name).collect();
        assert_eq!(unique.len(), 2_500);
    }

    #[test]
    fn missing_or_plain_file_is_not_listed() {
        let s = Scratch::new("missing");
        fs::write(s.0.join("f"), b"x").unwrap();
        let (mut buf, mut out) = (Vec::new(), Vec::new());
        assert!(!list(&s.0.join("nope"), &mut buf, &mut out));
        assert!(!list(&s.0.join("f"), &mut buf, &mut out));
        assert!(out.is_empty());
    }

    #[test]
    fn a_clone_shares_its_space() {
        let s = Scratch::new("clone");
        fs::write(s.0.join("orig.bin"), vec![b'y'; 2_000_000]).unwrap();
        // `cp -c` clones on APFS and fails elsewhere; there is then nothing to check.
        let cloned = std::process::Command::new("cp")
            .arg("-c")
            .arg(s.0.join("orig.bin"))
            .arg(s.0.join("clone.bin"))
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|st| st.success());
        if !cloned {
            eprintln!("skipped: this volume cannot clone files");
            return;
        }
        let (mut buf, mut out) = (Vec::new(), Vec::new());
        assert!(list(&s.0, &mut buf, &mut out));
        assert_eq!(out.len(), 2);
        for e in &out {
            assert!(e.allocated >= 2_000_000, "{e:?}");
            assert_eq!(e.private, 0, "deleting one of two clones frees nothing: {e:?}");
        }
    }
}
