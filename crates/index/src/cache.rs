//! The saved index: the parsed form of every file, with the size and
//! change time it had. At the next start only the files that changed are
//! parsed again; linking always runs.
//!
//! Each file is saved in two parts. The head (classes, methods, constants)
//! is read at start, because linking needs it. The body (references and
//! local variables) stays as bytes until a click or "find usages" needs
//! it. Most of the size is in the bodies.
//!
//! The saved file is mapped into memory, not read. The bodies stay in the
//! file, so the system can drop their pages when it needs memory. Rubyscope
//! never changes a saved file: `write` writes a new file and renames it, so
//! a map of the old file stays valid.
//!
//! The file is `MAGIC`, the length of the header, the header (postcard),
//! the head blob of each file, then the body blob of each file. The heads
//! can be read in parallel, and they are all together, so the start reads
//! no page of a body.

use std::collections::HashMap;
use std::fs::Metadata;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, bail};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::index::Index;
use crate::model::{LocalAssign, RawFile, RawRef};

const MAGIC: &[u8; 8] = b"RBSCOPE3";

/// Bytes that bodies point into: the mapped saved file, or the bytes of
/// one body that was just parsed.
type Bytes = Arc<dyn AsRef<[u8]> + Send + Sync>;

/// A fingerprint of the index code. A file saved by other code is not read.
const FINGERPRINT: &str = env!("RUBYSCOPE_INDEX_FINGERPRINT");

/// The size and change time of a file. A file with the same stamp is taken
/// as the same file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Stamp {
    pub size: u64,
    /// Nanoseconds since 1970.
    pub mtime: u64,
}

impl Stamp {
    pub fn of(meta: &Metadata) -> Stamp {
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as u64)
            .unwrap_or_default();
        Stamp { size: meta.len(), mtime }
    }
}

/// The parts of a parsed file that only clicks and usages need.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct BodyData {
    pub refs: Vec<RawRef>,
    pub locals: Vec<LocalAssign>,
}

/// The body of a file: decoded, or still as bytes in the saved index.
pub struct Body {
    data: OnceLock<BodyData>,
    saved: Option<(Bytes, Range<usize>)>,
}

impl std::fmt::Debug for Body {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.data.get().is_some() { "Body(decoded)" } else { "Body(saved)" })
    }
}

impl Body {
    pub fn new(data: BodyData) -> Body {
        Body { data: OnceLock::from(data), saved: None }
    }

    pub fn get(&self) -> &BodyData {
        self.data.get_or_init(|| match &self.saved {
            Some((bytes, range)) => postcard::from_bytes(&(**bytes).as_ref()[range.clone()]).unwrap_or_default(),
            None => BodyData::default(),
        })
    }

    /// Runs `f` on the body. A body that is still bytes is decoded for
    /// this call only, and not kept.
    pub fn with<R>(&self, f: impl FnOnce(&BodyData) -> R) -> R {
        if let Some(data) = self.data.get() {
            return f(data);
        }
        match &self.saved {
            Some((bytes, range)) => f(&postcard::from_bytes(&(**bytes).as_ref()[range.clone()]).unwrap_or_default()),
            None => f(&BodyData::default()),
        }
    }

    /// True when the body is decoded and kept in memory.
    pub fn is_decoded(&self) -> bool {
        self.data.get().is_some()
    }

    /// The saved bytes, or the body encoded again.
    fn bytes(&self) -> Result<std::borrow::Cow<'_, [u8]>> {
        Ok(match &self.saved {
            Some((bytes, range)) => std::borrow::Cow::Borrowed(&(**bytes).as_ref()[range.clone()]),
            None => std::borrow::Cow::Owned(postcard::to_stdvec(self.get())?),
        })
    }
}

/// Splits what extraction found into the head and the body.
pub fn split(mut raw: RawFile) -> (Arc<RawFile>, Arc<Body>) {
    let body = BodyData { refs: std::mem::take(&mut raw.refs), locals: std::mem::take(&mut raw.locals) };
    // Keep the body as bytes, the same as a body from the saved index. The
    // decoded form is about four times bigger, and most files are never
    // clicked.
    let Ok(bytes) = postcard::to_stdvec(&body) else { return (Arc::new(raw), Arc::new(Body::new(body))) };
    let len = bytes.len();
    (Arc::new(raw), Arc::new(Body { data: OnceLock::new(), saved: Some((Arc::new(bytes), 0..len)) }))
}

/// A parsed file that a build can take: its stamp, head and body.
pub type Parsed = (Stamp, Arc<RawFile>, Arc<Body>);

/// Parsed files that a build can take instead of parsing them again.
#[derive(Debug, Default)]
pub struct Reuse {
    pub files: HashMap<PathBuf, Parsed>,
    pub inflections: Option<Stamp>,
    /// The Ruby files of each fixed source (an installed gem), by its folder.
    pub lists: HashMap<PathBuf, Vec<PathBuf>>,
    /// True when the files come from the saved index.
    pub saved: bool,
}

impl Reuse {
    pub fn from_index(index: &Index) -> Reuse {
        let files = index.files.iter().map(|f| (f.path.clone(), (f.stamp, f.raw.clone(), f.body.clone()))).collect();
        Reuse { files, inflections: index.inflections, lists: fixed_lists(index), saved: false }
    }
}

/// The files of each fixed source of an index, by the source folder.
fn fixed_lists(index: &Index) -> HashMap<PathBuf, Vec<PathBuf>> {
    let mut lists: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    for f in &index.files {
        let source = &index.sources[f.source as usize];
        if source.fixed {
            lists.entry(source.root.clone()).or_default().push(f.path.clone());
        }
    }
    lists
}

#[derive(Serialize, Deserialize)]
struct Header {
    fingerprint: String,
    inflections: Option<Stamp>,
    /// (path, stamp, length of the head, length of the body)
    files: Vec<(PathBuf, Stamp, u64, u64)>,
    lists: HashMap<PathBuf, Vec<PathBuf>>,
}

/// `~/.cache/rubyscope`, or `$XDG_CACHE_HOME/rubyscope`.
pub fn default_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("rubyscope"))
}

/// The saved index of a project: `<folder name>-<hash of the path>.bin`.
pub fn file_for(cache_dir: &Path, project: &Path) -> PathBuf {
    let name = project.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in project.as_os_str().as_encoded_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    cache_dir.join(format!("{name}-{hash:016x}.bin"))
}

/// Reads a saved index. Fails when there is none, or when other code saved it.
pub fn read(path: &Path) -> Result<Reuse> {
    let file = std::fs::File::open(path).with_context(|| format!("reading {}", path.display()))?;
    // SAFETY: Rubyscope never changes a saved file in place (see the top of
    // this file). Every length in the file is checked before it is used.
    let map = Arc::new(unsafe { memmap2::Mmap::map(&file) }.with_context(|| format!("mapping {}", path.display()))?);
    let bytes: &[u8] = map.as_ref();
    if bytes.len() < 16 || &bytes[..8] != MAGIC {
        bail!("not a rubyscope index");
    }
    let header_len = u64::from_le_bytes(bytes[8..16].try_into()?) as usize;
    let header: Header = postcard::from_bytes(bytes.get(16..16 + header_len).context("short file")?)?;
    if header.fingerprint != FINGERPRINT {
        bail!("saved by other code");
    }
    let heads_start = 16 + header_len;
    let heads_len: u64 = header.files.iter().map(|f| f.2).sum();
    let (mut head_at, mut body_at) = (heads_start, heads_start + heads_len as usize);
    let mut entries = Vec::with_capacity(header.files.len());
    for (path, stamp, head_len, body_len) in header.files {
        let head = head_at..head_at + head_len as usize;
        let body = body_at..body_at + body_len as usize;
        anyhow::ensure!(body.end <= bytes.len(), "short file");
        (head_at, body_at) = (head.end, body.end);
        entries.push((path, stamp, head, body));
    }
    // A click reads the body of one file. Reading ahead would only load
    // the bodies of other files.
    #[cfg(unix)]
    let _ = map.advise(memmap2::Advice::Random);
    let files = entries
        .into_par_iter()
        .map(|(path, stamp, head, body)| {
            let raw: RawFile = postcard::from_bytes(&bytes[head])?;
            let body = Body { data: OnceLock::new(), saved: Some((map.clone() as Bytes, body)) };
            Ok((path, (stamp, Arc::new(raw), Arc::new(body))))
        })
        .collect::<Result<HashMap<_, _>>>()?;
    // The heads are decoded now, so their pages are not needed again.
    // SAFETY: the map is read-only, so the system reads the pages from the
    // file again if they are used.
    #[cfg(unix)]
    let _ = unsafe { map.unchecked_advise_range(memmap2::UncheckedAdvice::DontNeed, 0, head_at) };
    Ok(Reuse { files, inflections: header.inflections, lists: header.lists, saved: true })
}

/// Writes the parsed files of an index. Another file is written first and
/// then renamed, so a reader never sees half a file.
pub fn write(path: &Path, index: &Index) -> Result<()> {
    let blobs: Vec<(Vec<u8>, std::borrow::Cow<'_, [u8]>)> = index
        .files
        .par_iter()
        .map(|f| Ok((postcard::to_stdvec(f.raw.as_ref())?, f.body.bytes()?)))
        .collect::<Result<_>>()?;
    let header = Header {
        fingerprint: FINGERPRINT.to_string(),
        inflections: index.inflections,
        files: index
            .files
            .iter()
            .zip(&blobs)
            .map(|(f, (h, b))| (f.path.clone(), f.stamp, h.len() as u64, b.len() as u64))
            .collect(),
        lists: fixed_lists(index),
    };
    let header = postcard::to_stdvec(&header)?;
    let size = 16 + header.len() + blobs.iter().map(|(h, b)| h.len() + b.len()).sum::<usize>();
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(header.len() as u64).to_le_bytes());
    out.extend_from_slice(&header);
    for (h, _) in &blobs {
        out.extend_from_slice(h);
    }
    for (_, b) in &blobs {
        out.extend_from_slice(b);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, &out)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
