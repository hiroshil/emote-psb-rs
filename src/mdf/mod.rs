//! MDF (compressed PSB) reading and writing support.
//!
//! This module supports two MDF layouts used around E-mote/M2 PSB files:
//!
//! - plain MDF: `mdf\0` + original size + zlib payload + optional Adler-32;
//! - MT19937 MDF: same wrapper, but the zlib payload is XOR-obfuscated with a
//!   key stream derived from `MD5(key + original_file_name)`.
//!
//! The plain reader remains available through [`MdfReader::open`].  For `.psb.m`
//! files from games that use the MT19937 shell, use [`MdfReader::open_with_seed`]
//! or [`MdfReader::open_with_key`].

pub mod error;

use std::{
    io::{self, BufRead, Cursor, Read, Seek, SeekFrom, Write},
    path::Path,
};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use flate2::{read::ZlibDecoder, write::ZlibEncoder, Compression};

use crate::{
    mdf::error::{MdfCreateError, MdfOpenError},
    PSB_MDF_SIGNATURE,
};

const ZLIB_LEVEL_FAST: u8 = 0x9c;
const ZLIB_LEVEL_BEST: u8 = 0xda;
const ZLIB_LEVEL_NONE: u8 = 0x01;
const ZLIB_LEVEL_DEFAULT: u8 = 0x5e;
const DEFAULT_MT19937_KEY_LEN: usize = 0x83;

/// MDF shell detected/used by the reader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MdfShell {
    /// Plain zlib-compressed MDF.
    Plain,
    /// MDF whose zlib payload was XOR-obfuscated by an MT19937-derived key stream.
    Mt19937,
}

/// Options used when opening an MDF stream.
#[derive(Clone, Debug, Default)]
pub struct MdfReadOptions {
    /// Complete MT19937 seed.  In FreeMote terms this is usually `key + file_name`.
    pub seed: Option<Vec<u8>>,
    /// Game key.  This is combined with [`file_name`](Self::file_name).
    pub key: Option<String>,
    /// Original file name used by the game.  Required when `key` is used.
    pub file_name: Option<String>,
    /// Length of the repeated MT19937 XOR key.  FreeMote commonly uses `0x83`.
    pub key_len: Option<usize>,
    /// Whether to try plain MDF before MT19937 when a key/seed is supplied.
    pub try_plain_first: bool,
}

impl MdfReadOptions {
    /// Create empty options for a plain MDF.
    #[inline]
    pub const fn plain() -> Self {
        Self {
            seed: None,
            key: None,
            file_name: None,
            key_len: None,
            try_plain_first: true,
        }
    }

    /// Create options from a complete MT19937 seed.
    pub fn with_seed(seed: impl AsRef<[u8]>) -> Self {
        Self {
            seed: Some(seed.as_ref().to_vec()),
            key: None,
            file_name: None,
            key_len: Some(DEFAULT_MT19937_KEY_LEN),
            try_plain_first: true,
        }
    }

    /// Create options from a game key and the original file name.
    pub fn with_key(key: impl Into<String>, file_name: impl Into<String>) -> Self {
        Self {
            seed: None,
            key: Some(key.into()),
            file_name: Some(file_name.into()),
            key_len: Some(DEFAULT_MT19937_KEY_LEN),
            try_plain_first: true,
        }
    }

    /// Override the MT19937 key length.
    #[must_use]
    pub const fn key_len(mut self, key_len: usize) -> Self {
        self.key_len = Some(key_len);
        self
    }

    fn resolved_seed(&self) -> Result<Option<Vec<u8>>, MdfOpenError> {
        if let Some(seed) = &self.seed {
            return Ok(Some(seed.clone()));
        }

        match (&self.key, &self.file_name) {
            (Some(key), Some(file_name)) => {
                let mut seed = Vec::with_capacity(key.len() + file_name.len());
                seed.extend_from_slice(key.as_bytes());
                seed.extend_from_slice(file_name.as_bytes());
                Ok(Some(seed))
            }
            (Some(_), None) => Err(MdfOpenError::MissingMt19937Key),
            _ => Ok(None),
        }
    }

    fn resolved_key_len(&self) -> Result<usize, MdfOpenError> {
        let key_len = self.key_len.unwrap_or(DEFAULT_MT19937_KEY_LEN);
        if key_len == 0 {
            Err(MdfOpenError::InvalidKeyLength(key_len))
        } else {
            Ok(key_len)
        }
    }
}

/// A reader for MDF (zlib-compressed PSB) files.
///
/// The reader inflates the MDF payload during `open*` and then implements
/// [`Read`] over the decoded bytes.  The in-memory design lets the reader support
/// both plain zlib MDF and MT19937-obfuscated MDF while keeping the public API
/// simple.
pub struct MdfReader {
    inner: Cursor<Vec<u8>>,
    size: u32,
    shell: MdfShell,
}

impl MdfReader {
    /// Open a plain MDF stream.
    ///
    /// For MT19937 `.psb.m` files, use [`open_with_seed`](Self::open_with_seed),
    /// [`open_with_key`](Self::open_with_key), or [`open_with_options`](Self::open_with_options).
    pub fn open<T>(stream: T) -> Result<Self, MdfOpenError>
    where
        T: BufRead,
    {
        Self::open_with_options(stream, MdfReadOptions::plain())
    }

    /// Open an MT19937 MDF stream with a complete seed.
    ///
    /// The seed is usually `game_key + original_file_name`.
    pub fn open_with_seed<T>(stream: T, seed: impl AsRef<[u8]>, key_len: usize) -> Result<Self, MdfOpenError>
    where
        T: BufRead,
    {
        Self::open_with_options(
            stream,
            MdfReadOptions::with_seed(seed.as_ref()).key_len(key_len),
        )
    }

    /// Open an MT19937 MDF stream with a game key and original file name.
    pub fn open_with_key<T>(
        stream: T,
        key: impl Into<String>,
        file_name: impl Into<String>,
        key_len: usize,
    ) -> Result<Self, MdfOpenError>
    where
        T: BufRead,
    {
        Self::open_with_options(stream, MdfReadOptions::with_key(key, file_name).key_len(key_len))
    }

    /// Open an MT19937 MDF stream with a game key and a path.
    ///
    /// Only the final file name component is used for seed derivation.
    pub fn open_with_key_path<T, P>(
        stream: T,
        key: impl Into<String>,
        path: P,
        key_len: usize,
    ) -> Result<Self, MdfOpenError>
    where
        T: BufRead,
        P: AsRef<Path>,
    {
        let file_name = path
            .as_ref()
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(MdfOpenError::MissingMt19937Key)?;
        Self::open_with_key(stream, key, file_name, key_len)
    }

    /// Open an MDF stream with explicit read options.
    pub fn open_with_options<T>(mut stream: T, options: MdfReadOptions) -> Result<Self, MdfOpenError>
    where
        T: BufRead,
    {
        let signature = stream.read_u32::<LittleEndian>()?;
        if signature != PSB_MDF_SIGNATURE {
            return Err(MdfOpenError::InvalidSignature);
        }

        // In FreeMote-compatible MDF this is the original uncompressed length.
        // Older emote-psb-rs writers used it as compressed length.  Keeping the
        // raw header value preserves the old `size()` contract while decoding the
        // payload from the actual remaining bytes.
        let size = stream.read_u32::<LittleEndian>()?;

        let mut payload = Vec::new();
        stream.read_to_end(&mut payload)?;

        let seed = options.resolved_seed()?;
        let key_len = options.resolved_key_len()?;

        if options.try_plain_first {
            if let Ok(decoded) = inflate_zlib_payload(&payload) {
                return Ok(Self {
                    inner: Cursor::new(decoded),
                    size,
                    shell: MdfShell::Plain,
                });
            }
        }

        if let Some(seed) = seed {
            let decoded = decrypt_and_inflate_mt19937_payload(&payload, &seed, key_len)?;
            return Ok(Self {
                inner: Cursor::new(decoded),
                size,
                shell: MdfShell::Mt19937,
            });
        }

        // Return the real inflate error for callers that want a conventional MDF
        // failure rather than the heuristic MissingMt19937Key error.
        let decoded = inflate_zlib_payload(&payload).map_err(MdfOpenError::Decompress)?;
        Ok(Self {
            inner: Cursor::new(decoded),
            size,
            shell: MdfShell::Plain,
        })
    }

    /// Returns the raw size value stored in the MDF header.
    ///
    /// For FreeMote-compatible MDF this is the original uncompressed size.
    /// Older versions of this crate wrote the compressed payload size here.
    #[inline]
    pub const fn size(&self) -> u32 {
        self.size
    }

    /// Returns the detected MDF shell.
    #[inline]
    pub const fn shell(&self) -> MdfShell {
        self.shell
    }

    /// Returns the decoded payload length currently available from this reader.
    #[inline]
    pub fn decoded_len(&self) -> usize {
        self.inner.get_ref().len()
    }
}

impl Read for MdfReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

/// A writer for plain MDF (zlib-compressed PSB) files.
///
/// The writer emits a FreeMote-compatible header: `mdf\0`, original length,
/// then a complete zlib payload.  The zlib stream already contains its Adler-32
/// trailer.
pub struct MdfWriter<T> {
    stream: T,
    stream_start: u64,
    level: u8,
    buffer: Vec<u8>,
}

impl<T> MdfWriter<T>
where
    T: Write + Seek,
{
    /// Creates a new [`MdfWriter`], writing the MDF header placeholder to `stream`.
    ///
    /// - `stream` — writable, seekable output stream.
    /// - `level` — zlib compression level (0 = no compression, 9 = maximum).
    pub fn new(mut stream: T, level: u8) -> Result<Self, MdfCreateError> {
        stream.write_u32::<LittleEndian>(PSB_MDF_SIGNATURE)?;
        stream.write_u32::<LittleEndian>(0)?;
        let stream_start = stream.stream_position()?;

        Ok(Self {
            stream,
            stream_start,
            level,
            buffer: Vec::new(),
        })
    }

    /// Finish and return the wrapped output stream.
    pub fn finish(mut self) -> io::Result<T> {
        let original_len = self.buffer.len() as u32;

        {
            let mut encoder = ZlibEncoder::new(&mut self.stream, Compression::new(self.level as u32));
            encoder.write_all(&self.buffer)?;
            encoder.finish()?;
        }

        let end = self.stream.stream_position()?;

        self.stream.seek(SeekFrom::Start(self.stream_start - 4))?;
        self.stream.write_u32::<LittleEndian>(original_len)?;
        self.stream.seek(SeekFrom::Start(end))?;

        Ok(self.stream)
    }
}

impl<T> Write for MdfWriter<T>
where
    T: Write + Seek,
{
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn inflate_zlib_payload(payload: &[u8]) -> io::Result<Vec<u8>> {
    let mut decoder = ZlibDecoder::new(payload);
    let mut decoded = Vec::new();
    decoder.read_to_end(&mut decoded)?;
    Ok(decoded)
}

fn decrypt_and_inflate_mt19937_payload(
    payload: &[u8],
    seed: &[u8],
    key_len: usize,
) -> Result<Vec<u8>, MdfOpenError> {
    let variants = [
        MtSeedVariant::Md5ArrayLe,
        MtSeedVariant::Md5ArrayBe,
        MtSeedVariant::Md5FirstLe,
        MtSeedVariant::Md5FirstBe,
    ];
    let key_variants = [
        MtKeyVariant::CyclicLeWords,
        MtKeyVariant::CyclicBeWords,
        MtKeyVariant::StreamLeWords,
        MtKeyVariant::StreamBeWords,
    ];

    for seed_variant in variants {
        for key_variant in key_variants {
            let decrypted = mt19937_xor(payload, seed, key_len, seed_variant, key_variant);
            if !looks_like_zlib(&decrypted) {
                continue;
            }
            if let Ok(decoded) = inflate_zlib_payload(&decrypted) {
                return Ok(decoded);
            }
        }
    }

    Err(MdfOpenError::Decrypt)
}

fn looks_like_zlib(bytes: &[u8]) -> bool {
    if bytes.len() < 2 {
        return false;
    }

    let cmf = bytes[0];
    let flg = bytes[1];
    let method = cmf & 0x0f;
    let window = cmf >> 4;

    method == 8
        && window <= 7
        && (u16::from(cmf) * 256 + u16::from(flg)) % 31 == 0
        && matches!(flg, ZLIB_LEVEL_NONE | ZLIB_LEVEL_DEFAULT | ZLIB_LEVEL_FAST | ZLIB_LEVEL_BEST)
}

#[derive(Clone, Copy)]
enum MtSeedVariant {
    Md5ArrayLe,
    Md5ArrayBe,
    Md5FirstLe,
    Md5FirstBe,
}

#[derive(Clone, Copy)]
enum MtKeyVariant {
    CyclicLeWords,
    CyclicBeWords,
    StreamLeWords,
    StreamBeWords,
}

fn mt19937_xor(
    payload: &[u8],
    seed: &[u8],
    key_len: usize,
    seed_variant: MtSeedVariant,
    key_variant: MtKeyVariant,
) -> Vec<u8> {
    let digest = md5(seed);
    let mut mt = match seed_variant {
        MtSeedVariant::Md5ArrayLe => {
            let keys = [
                u32::from_le_bytes(digest[0..4].try_into().expect("slice length")),
                u32::from_le_bytes(digest[4..8].try_into().expect("slice length")),
                u32::from_le_bytes(digest[8..12].try_into().expect("slice length")),
                u32::from_le_bytes(digest[12..16].try_into().expect("slice length")),
            ];
            Mt19937::from_key_array(&keys)
        }
        MtSeedVariant::Md5ArrayBe => {
            let keys = [
                u32::from_be_bytes(digest[0..4].try_into().expect("slice length")),
                u32::from_be_bytes(digest[4..8].try_into().expect("slice length")),
                u32::from_be_bytes(digest[8..12].try_into().expect("slice length")),
                u32::from_be_bytes(digest[12..16].try_into().expect("slice length")),
            ];
            Mt19937::from_key_array(&keys)
        }
        MtSeedVariant::Md5FirstLe => Mt19937::new(u32::from_le_bytes(
            digest[0..4].try_into().expect("slice length"),
        )),
        MtSeedVariant::Md5FirstBe => Mt19937::new(u32::from_be_bytes(
            digest[0..4].try_into().expect("slice length"),
        )),
    };

    match key_variant {
        MtKeyVariant::CyclicLeWords => xor_with_cyclic_key(payload, key_len, &mut mt, true),
        MtKeyVariant::CyclicBeWords => xor_with_cyclic_key(payload, key_len, &mut mt, false),
        MtKeyVariant::StreamLeWords => xor_with_stream(payload, &mut mt, true),
        MtKeyVariant::StreamBeWords => xor_with_stream(payload, &mut mt, false),
    }
}

fn xor_with_cyclic_key(payload: &[u8], key_len: usize, mt: &mut Mt19937, little_endian: bool) -> Vec<u8> {
    let mut key = Vec::with_capacity(key_len);
    while key.len() < key_len {
        let n = mt.next_u32();
        let bytes = if little_endian {
            n.to_le_bytes()
        } else {
            n.to_be_bytes()
        };
        key.extend_from_slice(&bytes);
    }
    key.truncate(key_len);

    payload
        .iter()
        .enumerate()
        .map(|(idx, byte)| byte ^ key[idx % key.len()])
        .collect()
}

fn xor_with_stream(payload: &[u8], mt: &mut Mt19937, little_endian: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len());
    let mut key = [0u8; 4];
    let mut key_idx = 4;

    for byte in payload {
        if key_idx >= 4 {
            let n = mt.next_u32();
            key = if little_endian {
                n.to_le_bytes()
            } else {
                n.to_be_bytes()
            };
            key_idx = 0;
        }
        out.push(byte ^ key[key_idx]);
        key_idx += 1;
    }

    out
}

struct Mt19937 {
    mt: [u32; 624],
    index: usize,
}

impl Mt19937 {
    fn new(seed: u32) -> Self {
        let mut mt = [0u32; 624];
        mt[0] = seed;
        for i in 1..624 {
            mt[i] = 1_812_433_253u32
                .wrapping_mul(mt[i - 1] ^ (mt[i - 1] >> 30))
                .wrapping_add(i as u32);
        }
        Self { mt, index: 624 }
    }

    fn from_key_array(key: &[u32]) -> Self {
        let mut rng = Self::new(19_650_218);
        let mut i = 1usize;
        let mut j = 0usize;
        let mut k = 624usize.max(key.len());

        while k > 0 {
            rng.mt[i] = (rng.mt[i]
                ^ ((rng.mt[i - 1] ^ (rng.mt[i - 1] >> 30)).wrapping_mul(1_664_525)))
                .wrapping_add(key[j])
                .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= 624 {
                rng.mt[0] = rng.mt[623];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
            k -= 1;
        }

        k = 623;
        while k > 0 {
            rng.mt[i] = (rng.mt[i]
                ^ ((rng.mt[i - 1] ^ (rng.mt[i - 1] >> 30)).wrapping_mul(1_566_083_941)))
                .wrapping_sub(i as u32);
            i += 1;
            if i >= 624 {
                rng.mt[0] = rng.mt[623];
                i = 1;
            }
            k -= 1;
        }

        rng.mt[0] = 0x8000_0000;
        rng.index = 624;
        rng
    }

    fn next_u32(&mut self) -> u32 {
        const N: usize = 624;
        const M: usize = 397;
        const MATRIX_A: u32 = 0x9908_b0df;
        const UPPER_MASK: u32 = 0x8000_0000;
        const LOWER_MASK: u32 = 0x7fff_ffff;

        if self.index >= N {
            for kk in 0..(N - M) {
                let y = (self.mt[kk] & UPPER_MASK) | (self.mt[kk + 1] & LOWER_MASK);
                self.mt[kk] = self.mt[kk + M] ^ (y >> 1) ^ if y & 1 != 0 { MATRIX_A } else { 0 };
            }
            for kk in (N - M)..(N - 1) {
                let y = (self.mt[kk] & UPPER_MASK) | (self.mt[kk + 1] & LOWER_MASK);
                self.mt[kk] = self.mt[kk + M - N] ^ (y >> 1) ^ if y & 1 != 0 { MATRIX_A } else { 0 };
            }
            let y = (self.mt[N - 1] & UPPER_MASK) | (self.mt[0] & LOWER_MASK);
            self.mt[N - 1] = self.mt[M - 1] ^ (y >> 1) ^ if y & 1 != 0 { MATRIX_A } else { 0 };
            self.index = 0;
        }

        let mut y = self.mt[self.index];
        self.index += 1;

        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^= y >> 18;
        y
    }
}

fn md5(input: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20,
        5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23,
        4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15,
        21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76a_a478, 0xe8c7_b756, 0x2420_70db, 0xc1bd_ceee, 0xf57c_0faf, 0x4787_c62a,
        0xa830_4613, 0xfd46_9501, 0x6980_98d8, 0x8b44_f7af, 0xffff_5bb1, 0x895c_d7be,
        0x6b90_1122, 0xfd98_7193, 0xa679_438e, 0x49b4_0821, 0xf61e_2562, 0xc040_b340,
        0x265e_5a51, 0xe9b6_c7aa, 0xd62f_105d, 0x0244_1453, 0xd8a1_e681, 0xe7d3_fbc8,
        0x21e1_cde6, 0xc337_07d6, 0xf4d5_0d87, 0x455a_14ed, 0xa9e3_e905, 0xfcef_a3f8,
        0x676f_02d9, 0x8d2a_4c8a, 0xfffa_3942, 0x8771_f681, 0x6d9d_6122, 0xfde5_380c,
        0xa4be_ea44, 0x4bde_cfa9, 0xf6bb_4b60, 0xbebf_bc70, 0x289b_7ec6, 0xeaa1_27fa,
        0xd4ef_3085, 0x0488_1d05, 0xd9d4_d039, 0xe6db_99e5, 0x1fa2_7cf8, 0xc4ac_5665,
        0xf429_2244, 0x432a_ff97, 0xab94_23a7, 0xfc93_a039, 0x655b_59c3, 0x8f0c_cc92,
        0xffef_f47d, 0x8584_5dd1, 0x6fa8_7e4f, 0xfe2c_e6e0, 0xa301_4314, 0x4e08_11a1,
        0xf753_7e82, 0xbd3a_f235, 0x2ad7_d2bb, 0xeb86_d391,
    ];

    let mut msg = input.to_vec();
    let bit_len = (msg.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());

    let mut a0 = 0x6745_2301u32;
    let mut b0 = 0xefcd_ab89u32;
    let mut c0 = 0x98ba_dcfeu32;
    let mut d0 = 0x1032_5476u32;

    for chunk in msg.chunks_exact(64) {
        let mut m = [0u32; 16];
        for (i, word) in m.iter_mut().enumerate() {
            let start = i * 4;
            *word = u32::from_le_bytes(chunk[start..start + 4].try_into().expect("slice length"));
        }

        let mut a = a0;
        let mut b = b0;
        let mut c = c0;
        let mut d = d0;

        for i in 0..64 {
            let (f, g) = if i < 16 {
                ((b & c) | ((!b) & d), i)
            } else if i < 32 {
                ((d & b) | ((!d) & c), (5 * i + 1) % 16)
            } else if i < 48 {
                (b ^ c ^ d, (3 * i + 5) % 16)
            } else {
                (c ^ (b | (!d)), (7 * i) % 16)
            };

            let temp = d;
            d = c;
            c = b;
            b = b.wrapping_add(
                a.wrapping_add(f)
                    .wrapping_add(K[i])
                    .wrapping_add(m[g])
                    .rotate_left(S[i]),
            );
            a = temp;
        }

        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_empty() {
        assert_eq!(md5(b""), [
            0xd4, 0x1d, 0x8c, 0xd9, 0x8f, 0x00, 0xb2, 0x04,
            0xe9, 0x80, 0x09, 0x98, 0xec, 0xf8, 0x42, 0x7e,
        ]);
    }

    #[test]
    fn mt19937_reference_seed_5489() {
        let mut mt = Mt19937::new(5489);
        assert_eq!(mt.next_u32(), 3_499_211_612);
        assert_eq!(mt.next_u32(), 581_869_302);
    }
}
