//! "Fast start" for uploaded MP4s: move the `moov` box ahead of `mdat`.
//!
//! An MP4 is a sequence of boxes. The player needs `moov` (the index: which
//! bytes are which frame) before it can show a single picture, but most
//! encoders write it *last*, after `mdat` (the frames), because that is the
//! order the numbers are known in. Over HTTP that means the browser fetches
//! the start of the file, finds no index, and has to range-request the tail
//! before it can begin -- a whole extra round trip, visible as the poster
//! sitting there for a beat longer than it should on every clip.
//!
//! This does what `ffmpeg -movflags +faststart` does, without ffmpeg: cut
//! `moov` out, paste it after `ftyp`, and add the size of `moov` to every
//! chunk offset that pointed past the insertion point. Nothing is decoded and
//! nothing about the video itself changes; the file's bytes are only
//! reordered, and `relocate` returns `None` -- keep the original -- the moment
//! the structure is anything other than the plain case it understands.
//!
//! The fingerprint is taken *after* this runs, so the same clip uploaded once
//! from a phone (moov last) and once after passing through a tool that
//! already fast-started it (moov first) dedupes as the same bytes -- as far as
//! the reorder goes; a re-encode is a different file and always was.

/// The container boxes chunk-offset tables live inside, from the top down.
/// Anything else (`edts`, `udta`, `meta`...) holds no file offsets and is
/// copied through untouched.
const CONTAINERS: [&[u8; 4]; 5] = [b"moov", b"trak", b"mdia", b"minf", b"stbl"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Box_ {
    /// Absolute offset of the size field.
    start: usize,
    /// Bytes of header before the payload: 8, or 16 with a 64-bit size.
    header: usize,
    /// Total size including the header.
    size: usize,
    kind: [u8; 4],
}

impl Box_ {
    fn end(self) -> usize {
        self.start + self.size
    }
}

/// Read the box header at `pos`. `None` for anything that does not fit:
/// truncated header, a size smaller than its own header, a size that runs
/// past the end of the data. `size == 0` ("to end of file") is only accepted
/// for a box that really is last, which is the only place it is meaningful.
fn read_box(data: &[u8], pos: usize) -> Option<Box_> {
    let hdr = data.get(pos..pos + 8)?;
    let size32 = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as usize;
    let kind = [hdr[4], hdr[5], hdr[6], hdr[7]];
    let (header, size) = match size32 {
        0 => (8, data.len() - pos),
        1 => {
            let big = data.get(pos + 8..pos + 16)?;
            let size = u64::from_be_bytes(big.try_into().ok()?);
            (16, usize::try_from(size).ok()?)
        }
        n => (8, n),
    };
    if size < header || pos.checked_add(size)? > data.len() {
        return None;
    }
    Some(Box_ {
        start: pos,
        header,
        size,
        kind,
    })
}

/// Every box in `data[from..to]`, in order, or `None` if the run does not
/// tile the range exactly.
fn boxes(data: &[u8], from: usize, to: usize) -> Option<Vec<Box_>> {
    let mut out = Vec::new();
    let mut pos = from;
    while pos < to {
        let b = read_box(&data[..to], pos)?;
        out.push(b);
        pos = b.end();
    }
    (pos == to).then_some(out)
}

/// Return the file with `moov` moved ahead of `mdat`, or `None` when it is
/// already there, when the layout is anything the walk does not understand,
/// or when an adjusted offset would not fit its field. `None` always means
/// "store what was uploaded"; it is never an error.
pub fn relocate(data: &[u8]) -> Option<Vec<u8>> {
    let top = boxes(data, 0, data.len())?;
    let ftyp = top.first().filter(|b| &b.kind == b"ftyp")?;
    let moov = *top.iter().find(|b| &b.kind == b"moov")?;
    let mdat = *top.iter().find(|b| &b.kind == b"mdat")?;
    if top.iter().any(|b| &b.kind == b"moof") {
        // Fragmented: the index is spread through the file by design and the
        // offsets in each fragment are relative. Nothing to move.
        return None;
    }
    if moov.start < mdat.start {
        return None;
    }
    // A 64-bit-sized moov would need a 64-bit-sized copy; not worth the case.
    if moov.header != 8 {
        return None;
    }

    let insert_at = ftyp.end();
    let shift = moov.size;
    // The bytes that move are exactly those between the insertion point and
    // where moov used to start. A chunk offset inside that window grows by
    // `shift`; one after moov's old end lands where it always was.
    let moved = insert_at..moov.start;

    let mut moov_bytes = data[moov.start..moov.end()].to_vec();
    // A trailing moov may carry size 0 ("to end of file"); once it is in the
    // middle of the file that reading is wrong, so write its real size.
    if moov_bytes[..4] == [0, 0, 0, 0] {
        let real = u32::try_from(moov.size).ok()?;
        moov_bytes[..4].copy_from_slice(&real.to_be_bytes());
    }
    // Offsets inside the copy are relative to the copy; every box position
    // found below is an index into `moov_bytes`.
    let moov_len = moov_bytes.len();
    patch_offsets(&mut moov_bytes, moov.header, moov_len, |off| {
        let off = usize::try_from(off).ok()?;
        if off >= data.len() {
            return None;
        }
        let new = if moved.contains(&off) {
            off + shift
        } else {
            off
        };
        u64::try_from(new).ok()
    })?;

    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(&data[..insert_at]);
    out.extend_from_slice(&moov_bytes);
    out.extend_from_slice(&data[insert_at..moov.start]);
    out.extend_from_slice(&data[moov.end()..]);
    debug_assert_eq!(out.len(), data.len());
    Some(out)
}

/// Walk the children of the container occupying `buf[from..to]` (payload
/// only: `from` is past the header) and rewrite every `stco`/`co64` entry
/// through `f`. `None` if any box is malformed or `f` declines an offset.
fn patch_offsets(
    buf: &mut [u8],
    from: usize,
    to: usize,
    f: impl Fn(u64) -> Option<u64> + Copy,
) -> Option<()> {
    let children = boxes(buf, from, to)?;
    for b in children {
        let payload = b.start + b.header;
        if CONTAINERS.contains(&&b.kind) {
            patch_offsets(buf, payload, b.end(), f)?;
        } else if &b.kind == b"stco" || &b.kind == b"co64" {
            let wide = &b.kind == b"co64";
            // version(1) flags(3) entry_count(4)
            let count_at = payload + 4;
            let count =
                u32::from_be_bytes(buf.get(count_at..count_at + 4)?.try_into().ok()?) as usize;
            let width = if wide { 8 } else { 4 };
            let entries_at = count_at + 4;
            if entries_at + count * width > b.end() {
                return None;
            }
            for i in 0..count {
                let at = entries_at + i * width;
                let old = if wide {
                    u64::from_be_bytes(buf[at..at + 8].try_into().ok()?)
                } else {
                    u64::from(u32::from_be_bytes(buf[at..at + 4].try_into().ok()?))
                };
                let new = f(old)?;
                if wide {
                    buf[at..at + 8].copy_from_slice(&new.to_be_bytes());
                } else {
                    let new = u32::try_from(new).ok()?;
                    buf[at..at + 4].copy_from_slice(&new.to_be_bytes());
                }
            }
        }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(payload);
        v
    }

    fn stco(offsets: &[u32]) -> Vec<u8> {
        let mut p = vec![0, 0, 0, 0];
        p.extend((offsets.len() as u32).to_be_bytes());
        for o in offsets {
            p.extend(o.to_be_bytes());
        }
        bx(b"stco", &p)
    }

    fn co64(offsets: &[u64]) -> Vec<u8> {
        let mut p = vec![0, 0, 0, 0];
        p.extend((offsets.len() as u32).to_be_bytes());
        for o in offsets {
            p.extend(o.to_be_bytes());
        }
        bx(b"co64", &p)
    }

    fn ftyp() -> Vec<u8> {
        bx(b"ftyp", b"isom\0\0\x02\0isomiso2")
    }

    /// moov > trak > mdia > minf > stbl > { stco, co64 }, plus a `udta`
    /// sibling holding something that looks like an offset but is not.
    fn moov(stco_offs: &[u32], co64_offs: &[u64]) -> Vec<u8> {
        let mut stbl = stco(stco_offs);
        stbl.extend(co64(co64_offs));
        let minf = bx(b"minf", &bx(b"stbl", &stbl));
        let mdia = bx(b"mdia", &minf);
        let trak = bx(b"trak", &mdia);
        let mut inner = trak;
        inner.extend(bx(b"udta", &[0xff; 12]));
        bx(b"moov", &inner)
    }

    #[test]
    fn moov_moves_ahead_and_offsets_follow_the_frames() {
        let frames: Vec<u8> = (0u8..32).collect();
        let head = ftyp();
        let mdat = bx(b"mdat", &frames);
        // Two chunks in mdat: at payload+0 and payload+16.
        let first = (head.len() + 8) as u32;
        let second = first + 16;
        let idx = moov(&[first, second], &[u64::from(second)]);
        let mut file = head.clone();
        file.extend(&mdat);
        file.extend(&idx);

        let out = relocate(&file).expect("plain moov-last file relocates");
        assert_eq!(out.len(), file.len());
        let top = boxes(&out, 0, out.len()).unwrap();
        let kinds: Vec<&[u8; 4]> = top.iter().map(|b| &b.kind).collect();
        assert_eq!(kinds, [b"ftyp", b"moov", b"mdat"]);

        // Every recorded offset now lands on the same frame byte it did before.
        let shift = idx.len();
        for (old, new) in [
            (first, first + shift as u32),
            (second, second + shift as u32),
        ] {
            assert_eq!(out[new as usize], file[old as usize]);
        }
        // And the udta payload survived untouched.
        assert!(out.windows(12).any(|w| w == [0xff; 12]));
        // Running again is a no-op: it is already fast.
        assert_eq!(relocate(&out), None);
    }

    #[test]
    fn already_fast_files_are_left_alone() {
        let mut file = ftyp();
        file.extend(moov(&[40], &[]));
        file.extend(bx(b"mdat", &[1, 2, 3, 4]));
        assert_eq!(relocate(&file), None);
    }

    #[test]
    fn fragmented_and_malformed_input_is_declined() {
        let mut frag = ftyp();
        frag.extend(bx(b"mdat", &[0; 4]));
        frag.extend(bx(b"moof", &[0; 4]));
        frag.extend(moov(&[0], &[]));
        assert_eq!(relocate(&frag), None);

        assert_eq!(relocate(b"not an mp4 at all"), None);
        assert_eq!(relocate(&[]), None);

        // A size that overruns the file.
        let mut bad = ftyp();
        bad.extend(bx(b"mdat", &[0; 4]));
        let mut lying = moov(&[0], &[]);
        lying[0..4].copy_from_slice(&0xffff_u32.to_be_bytes());
        bad.extend(lying);
        assert_eq!(relocate(&bad), None);

        // An offset pointing past the end of the data means the index is not
        // describing this file; keep the bytes as uploaded.
        let mut wild = ftyp();
        wild.extend(bx(b"mdat", &[0; 4]));
        wild.extend(moov(&[1 << 30], &[]));
        assert_eq!(relocate(&wild), None);
    }

    #[test]
    fn a_zero_size_last_box_is_understood() {
        let mut file = ftyp();
        file.extend(bx(b"mdat", &[9; 8]));
        let mut last = moov(&[(file.len() - 8) as u32], &[]);
        last[0..4].copy_from_slice(&[0, 0, 0, 0]);
        file.extend(last);
        let out = relocate(&file).expect("zero-size trailing moov");
        let top = boxes(&out, 0, out.len()).unwrap();
        assert_eq!(&top[1].kind, b"moov");
        // The copy in the middle of the file carries its real size, not 0.
        assert_eq!(top[1].size, file.len() - top[1].start - 16);
        assert_eq!(&top[2].kind, b"mdat");
    }
}
