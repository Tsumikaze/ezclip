//! Small MP4 writing toolkit shared by clip saving (`recorder/mux.rs`) and
//! trimming (`clipfile.rs`): box builders and a track's sample tables.
//! Pure Rust, no Windows calls.

pub fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(body.len() + 8);
    v.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes());
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}

pub fn full(kind: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(body.len() + 4);
    b.push(version);
    b.extend_from_slice(&flags.to_be_bytes()[1..]);
    b.extend_from_slice(body);
    bx(kind, &b)
}

pub fn cat(parts: &[Vec<u8>]) -> Vec<u8> {
    parts.concat()
}

/// Big-endian byte builder.
pub struct W(pub Vec<u8>);
impl W {
    pub fn new() -> Self {
        W(Vec::new())
    }
    pub fn u8(mut self, v: u8) -> Self {
        self.0.push(v);
        self
    }
    pub fn u16(mut self, v: u16) -> Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn u32(mut self, v: u32) -> Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn u64(mut self, v: u64) -> Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn bytes(mut self, b: &[u8]) -> Self {
        self.0.extend_from_slice(b);
        self
    }
    pub fn zeros(mut self, n: usize) -> Self {
        self.0.resize(self.0.len() + n, 0);
        self
    }
}

const MATRIX: [u32; 9] = [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000];

pub fn matrix(mut w: W) -> W {
    for m in MATRIX {
        w = w.u32(m);
    }
    w
}

/// `ftyp` with the given compatible brands.
pub fn ftyp(brands: &[&[u8; 4]]) -> Vec<u8> {
    let mut w = W::new().bytes(b"isom").u32(512);
    for b in brands {
        w = w.bytes(&b[..]);
    }
    bx(b"ftyp", &w.0)
}

/// 64-bit `mdat` header; patch the size at `start + 8` once all samples are written.
pub fn mdat_header() -> Vec<u8> {
    W::new().u32(1).bytes(b"mdat").u64(0).0
}

pub fn mvhd(movie_ms: u32, next_track_id: u32) -> Vec<u8> {
    full(
        b"mvhd",
        0,
        0,
        &matrix(W::new().u32(0).u32(0).u32(1000).u32(movie_ms).u32(0x0001_0000).u16(0x0100).zeros(10))
            .zeros(24)
            .u32(next_track_id)
            .0,
    )
}

/// `stsd` holding one sample entry (e.g. `avc1` with its `avcC`).
pub fn stsd(entry: Vec<u8>) -> Vec<u8> {
    full(b"stsd", 0, 0, &cat(&[W::new().u32(1).0, entry]))
}

pub enum TrackKind {
    Video { width: u32, height: u32 },
    Audio,
}

/// One track's samples as written to `mdat`.
#[derive(Default)]
pub struct Track {
    pub durations: Vec<u32>,
    pub sizes: Vec<u32>,
    pub offsets: Vec<u64>,
    /// 1-based numbers of keyframes; `None` = every sample is a keyframe (no `stss`).
    pub sync: Option<Vec<u32>>,
    /// Composition offsets (B-frames); `None` = none (no `ctts`).
    pub ctts: Option<Vec<i32>>,
}

impl Track {
    pub fn duration(&self) -> u64 {
        self.durations.iter().map(|&d| d as u64).sum()
    }

    pub fn push(&mut self, offset: u64, size: u32, duration: u32) {
        self.offsets.push(offset);
        self.sizes.push(size);
        self.durations.push(duration);
    }

    fn stbl(&self, stsd_box: Vec<u8>) -> Vec<u8> {
        let mut parts = vec![stsd_box];
        // stts: run-length of durations
        let mut runs: Vec<(u32, u32)> = Vec::new();
        for &d in &self.durations {
            match runs.last_mut() {
                Some((n, dd)) if *dd == d => *n += 1,
                _ => runs.push((1, d)),
            }
        }
        let mut w = W::new().u32(runs.len() as u32);
        for (n, d) in runs {
            w = w.u32(n).u32(d);
        }
        parts.push(full(b"stts", 0, 0, &w.0));
        if let Some(ctts) = &self.ctts {
            let mut runs: Vec<(u32, i32)> = Vec::new();
            for &o in ctts {
                match runs.last_mut() {
                    Some((n, oo)) if *oo == o => *n += 1,
                    _ => runs.push((1, o)),
                }
            }
            let version = if ctts.iter().any(|&o| o < 0) { 1 } else { 0 };
            let mut w = W::new().u32(runs.len() as u32);
            for (n, o) in runs {
                w = w.u32(n).u32(o as u32);
            }
            parts.push(full(b"ctts", version, 0, &w.0));
        }
        if let Some(sync) = &self.sync {
            let mut w = W::new().u32(sync.len() as u32);
            for &s in sync {
                w = w.u32(s);
            }
            parts.push(full(b"stss", 0, 0, &w.0));
        }
        // one sample per chunk keeps stsc trivial
        parts.push(full(b"stsc", 0, 0, &W::new().u32(1).u32(1).u32(1).u32(1).0));
        let mut w = W::new().u32(0).u32(self.sizes.len() as u32);
        for &s in &self.sizes {
            w = w.u32(s);
        }
        parts.push(full(b"stsz", 0, 0, &w.0));
        if self.offsets.iter().any(|&o| o > u32::MAX as u64) {
            let mut w = W::new().u32(self.offsets.len() as u32);
            for &o in &self.offsets {
                w = w.u64(o);
            }
            parts.push(full(b"co64", 0, 0, &w.0));
        } else {
            let mut w = W::new().u32(self.offsets.len() as u32);
            for &o in &self.offsets {
                w = w.u32(o as u32);
            }
            parts.push(full(b"stco", 0, 0, &w.0));
        }
        bx(b"stbl", &cat(&parts))
    }

    /// The complete `trak` box. `language` is the packed ISO-639 code from `mdhd` (0x55C4 = "und").
    pub fn trak(&self, id: u32, timescale: u32, movie_ms: u32, kind: TrackKind, stsd_box: Vec<u8>, language: u16) -> Vec<u8> {
        let (video, w, h) = match kind {
            TrackKind::Video { width, height } => (true, width, height),
            TrackKind::Audio => (false, 0, 0),
        };
        let tkhd = full(
            b"tkhd",
            0,
            3, // enabled, in movie
            &matrix(
                W::new()
                    .u32(0)
                    .u32(0)
                    .u32(id)
                    .u32(0)
                    .u32(movie_ms)
                    .zeros(8)
                    .u16(0)
                    .u16(if video { 0 } else { 1 })
                    .u16(if video { 0 } else { 0x0100 })
                    .u16(0),
            )
            .u32(w << 16)
            .u32(h << 16)
            .0,
        );
        let mdhd = full(b"mdhd", 0, 0, &W::new().u32(0).u32(0).u32(timescale).u32(self.duration() as u32).u16(language).u16(0).0);
        let (handler, name): (&[u8; 4], &[u8]) = if video { (b"vide", b"VideoHandler\0") } else { (b"soun", b"SoundHandler\0") };
        let hdlr = full(b"hdlr", 0, 0, &W::new().u32(0).bytes(handler).zeros(12).bytes(name).0);
        let xmhd = if video { full(b"vmhd", 0, 1, &[0; 8]) } else { full(b"smhd", 0, 0, &[0; 4]) };
        let dref = full(b"dref", 0, 0, &cat(&[W::new().u32(1).0, full(b"url ", 0, 1, &[])]));
        let dinf = bx(b"dinf", &dref);
        let minf = bx(b"minf", &cat(&[xmhd, dinf, self.stbl(stsd_box)]));
        let mdia = bx(b"mdia", &cat(&[mdhd, hdlr, minf]));
        bx(b"trak", &cat(&[tkhd, mdia]))
    }
}
