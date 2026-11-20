//! What a file is, decided from its content against an allow-list (CMS-261), and how large an
//! image becomes when it is decoded (CMS-263).
//!
//! The declared type and the file name are never consulted: an upload is accepted only when its
//! content is one of the accepted types and that type is the one declared. Executables,
//! archives and markup (SVG, HTML, XML) are refused by name, so the refusal says why.

/// How many bytes of the start of a file are read to decide what it is. A JPEG's frame header
/// can follow an EXIF segment of up to 64 KiB, so this is more than that.
pub const HEAD_BYTES: usize = 256 * 1024;

/// One accepted media type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Accepted {
    pub media_type: &'static str,
    /// Whether it may be shown inline. A type that can carry script (PDF) is served as an
    /// attachment only.
    pub inline: bool,
    /// The extension a download of it is named with.
    pub extension: &'static str,
}

const PNG: Accepted = Accepted {
    media_type: "image/png",
    inline: true,
    extension: "png",
};
const JPEG: Accepted = Accepted {
    media_type: "image/jpeg",
    inline: true,
    extension: "jpg",
};
const GIF: Accepted = Accepted {
    media_type: "image/gif",
    inline: true,
    extension: "gif",
};
const WEBP: Accepted = Accepted {
    media_type: "image/webp",
    inline: true,
    extension: "webp",
};
const PDF: Accepted = Accepted {
    media_type: "application/pdf",
    inline: false,
    extension: "pdf",
};

/// Every accepted type.
pub const ACCEPTED: [Accepted; 5] = [PNG, JPEG, GIF, WEBP, PDF];

/// The accepted type of a media type name, when it is one.
pub fn accepted(media_type: &str) -> Option<Accepted> {
    ACCEPTED
        .iter()
        .copied()
        .find(|a| a.media_type == media_type)
}

/// What the content of a file is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sniffed {
    pub kind: Accepted,
    /// The width and height of an image, as its header states them.
    pub dimensions: Option<(u64, u64)>,
}

fn starts(head: &[u8], magic: &[u8]) -> bool {
    head.len() >= magic.len() && &head[..magic.len()] == magic
}

fn be16(head: &[u8], at: usize) -> Option<u64> {
    Some(u64::from(u16::from_be_bytes([
        *head.get(at)?,
        *head.get(at + 1)?,
    ])))
}

fn le16(head: &[u8], at: usize) -> Option<u64> {
    Some(u64::from(u16::from_le_bytes([
        *head.get(at)?,
        *head.get(at + 1)?,
    ])))
}

fn be32(head: &[u8], at: usize) -> Option<u64> {
    Some(u64::from(u32::from_be_bytes([
        *head.get(at)?,
        *head.get(at + 1)?,
        *head.get(at + 2)?,
        *head.get(at + 3)?,
    ])))
}

fn le24(head: &[u8], at: usize) -> Option<u64> {
    Some(
        u64::from(*head.get(at)?)
            | u64::from(*head.get(at + 1)?) << 8
            | u64::from(*head.get(at + 2)?) << 16,
    )
}

/// Why content that is not accepted is refused, naming what it is when that is known.
fn refusal(head: &[u8]) -> String {
    let executable = starts(head, b"MZ")
        || starts(head, b"\x7fELF")
        || starts(head, b"#!")
        || starts(head, &[0xfe, 0xed, 0xfa, 0xce])
        || starts(head, &[0xfe, 0xed, 0xfa, 0xcf])
        || starts(head, &[0xce, 0xfa, 0xed, 0xfe])
        || starts(head, &[0xcf, 0xfa, 0xed, 0xfe])
        || starts(head, &[0xca, 0xfe, 0xba, 0xbe])
        || starts(head, b"\0asm");
    if executable {
        return "The content is an executable program, which is never accepted.".into();
    }
    let archive = starts(head, b"PK\x03\x04")
        || starts(head, &[0x1f, 0x8b])
        || starts(head, b"7z\xbc\xaf\x27\x1c")
        || starts(head, b"Rar!")
        || starts(head, b"BZh");
    if archive {
        return "The content is an archive, which is not accepted.".into();
    }
    let text = head.strip_prefix(b"\xef\xbb\xbf").unwrap_or(head);
    let first = text.iter().find(|b| !b.is_ascii_whitespace());
    if first == Some(&b'<') {
        return "The content is markup (HTML, SVG or XML), which is not accepted: it can carry script.".into();
    }
    format!(
        "The content is not one of the accepted types ({}).",
        ACCEPTED
            .iter()
            .map(|a| a.media_type)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// The width and height a JPEG's first frame header states.
fn jpeg_dimensions(head: &[u8]) -> Option<(u64, u64)> {
    let mut at = 2;
    loop {
        // Skip fill bytes before a marker.
        while head.get(at) == Some(&0xff) && head.get(at + 1) == Some(&0xff) {
            at += 1;
        }
        if head.get(at) != Some(&0xff) {
            return None;
        }
        let marker = *head.get(at + 1)?;
        match marker {
            0xd8 | 0x01 | 0xd0..=0xd7 => at += 2,
            0xd9 | 0xda => return None,
            0xc0..=0xcf if !matches!(marker, 0xc4 | 0xc8 | 0xcc) => {
                let height = be16(head, at + 5)?;
                let width = be16(head, at + 7)?;
                return Some((width, height));
            }
            _ => {
                let length = usize::try_from(be16(head, at + 2)?).ok()?;
                if length < 2 {
                    return None;
                }
                at += 2 + length;
            }
        }
    }
}

/// The width and height a WebP header states.
fn webp_dimensions(head: &[u8]) -> Option<(u64, u64)> {
    match head.get(12..16)? {
        b"VP8X" => Some((le24(head, 24)? + 1, le24(head, 27)? + 1)),
        b"VP8 " => {
            if head.get(23..26) != Some(&[0x9d_u8, 0x01, 0x2a][..]) {
                return None;
            }
            Some((le16(head, 26)? & 0x3fff, le16(head, 28)? & 0x3fff))
        }
        b"VP8L" => {
            if *head.get(20)? != 0x2f {
                return None;
            }
            let bits = u64::from(u32::from_le_bytes([
                *head.get(21)?,
                *head.get(22)?,
                *head.get(23)?,
                *head.get(24)?,
            ]));
            Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
        }
        _ => None,
    }
}

/// What the start of a file is, or why it is refused.
pub fn sniff(head: &[u8]) -> Result<Sniffed, String> {
    let unreadable = |kind: &str| format!("The size of the {kind} image could not be read.");
    if starts(head, b"\x89PNG\r\n\x1a\n") {
        if head.get(12..16) != Some(b"IHDR".as_slice()) {
            return Err(unreadable("PNG"));
        }
        let dimensions = be32(head, 16).zip(be32(head, 20));
        return match dimensions {
            Some(d) => Ok(Sniffed {
                kind: PNG,
                dimensions: Some(d),
            }),
            None => Err(unreadable("PNG")),
        };
    }
    if starts(head, b"GIF87a") || starts(head, b"GIF89a") {
        return match le16(head, 6).zip(le16(head, 8)) {
            Some(d) => Ok(Sniffed {
                kind: GIF,
                dimensions: Some(d),
            }),
            None => Err(unreadable("GIF")),
        };
    }
    if starts(head, &[0xff, 0xd8, 0xff]) {
        return match jpeg_dimensions(head) {
            Some(d) => Ok(Sniffed {
                kind: JPEG,
                dimensions: Some(d),
            }),
            None => Err(unreadable("JPEG")),
        };
    }
    if starts(head, b"RIFF") && head.get(8..12) == Some(b"WEBP".as_slice()) {
        return match webp_dimensions(head) {
            Some(d) => Ok(Sniffed {
                kind: WEBP,
                dimensions: Some(d),
            }),
            None => Err(unreadable("WebP")),
        };
    }
    if starts(head, b"%PDF-") {
        return Ok(Sniffed {
            kind: PDF,
            dimensions: None,
        });
    }
    Err(refusal(head))
}

/// A declared media type in the form it is compared in: lower case, without parameters, with
/// the common aliases of JPEG folded.
pub fn normalise(declared: &str) -> String {
    let base = declared
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match base.as_str() {
        "image/jpg" | "image/pjpeg" => "image/jpeg".into(),
        _ => base,
    }
}

/// Whether a field that accepts `allowed` accepts `media_type`. An entry is a media type or
/// `type/*`; a field that lists none accepts every accepted type.
pub fn field_accepts(allowed: &[String], media_type: &str) -> bool {
    allowed.is_empty()
        || allowed.iter().any(|entry| {
            let entry = normalise(entry);
            match entry.strip_suffix("/*") {
                Some(major) => media_type
                    .split_once('/')
                    .is_some_and(|(have, _)| have == major),
                None => entry == media_type,
            }
        })
}

/// Check content against its declared type, the types a field accepts and the decompressed
/// limit: the accepted type, or why the content is refused (CMS-261, CMS-263).
pub fn check(
    head: &[u8],
    declared: &str,
    field_types: Option<&[String]>,
    max_decompressed: u64,
) -> Result<Accepted, String> {
    let sniffed = sniff(head)?;
    let declared = normalise(declared);
    if declared != sniffed.kind.media_type {
        return Err(format!(
            "The content is {}, not the declared {declared}.",
            sniffed.kind.media_type
        ));
    }
    if let Some(allowed) = field_types {
        if !field_accepts(allowed, sniffed.kind.media_type) {
            return Err(format!(
                "The field does not accept {}.",
                sniffed.kind.media_type
            ));
        }
    }
    if let Some((width, height)) = sniffed.dimensions {
        let decoded = width.saturating_mul(height).saturating_mul(4);
        if width == 0 || height == 0 || decoded > max_decompressed {
            return Err(format!(
                "The image is {width} by {height} pixels; decoded it would exceed the limit of {max_decompressed} bytes."
            ));
        }
    }
    Ok(sniffed.kind)
}
